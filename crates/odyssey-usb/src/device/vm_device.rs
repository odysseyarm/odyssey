use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use nusb::transfer::{Bulk, In, Out};
use nusb::{DeviceInfo, Interface};
use odyssey_protocol::control::device::TransportMode;
use odyssey_protocol::{PocMarkersReport, VendorData};
use opencv_ros_camera::RosOpenCvIntrinsics;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt as _};
use tracing::{debug, info, instrument, trace, warn};

use super::general_settings::GeneralSettings;
use super::mux_device::MuxDevice;
use super::packet_stream::PacketStream;
use super::packet_transport::{PacketTransport, PacketTransportTx};
use super::response_slots::{ResponseChannel, ResponseSlot, State, StreamsActive};
use super::retry::retry;
use crate::packets::vm::{
    AccelReport, BatteryReport, CombinedMarkersReport, ConfigKind, GeneralConfig, ImpactReport,
    MotData, ObjectReport, Packet, PacketData, PacketType, Port, Props, Register, StreamUpdate,
    WriteRegister,
};

mod control_endpoint;
mod registers;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub struct VmDevice {
    transport: PacketTransportTx,
    thread_state: Weak<State>,
    cancel: Arc<tokio_util::sync::CancellationToken>, // Signals dispatcher to exit when VmDevice drops
    ctrl_if: Option<Interface>,
    transport_cancel: Arc<tokio_util::sync::CancellationToken>, // Signals mux reader to exit when VmDevice drops
}

impl Drop for VmDevice {
    fn drop(&mut self) {
        eprintln!("!!! VmDevice DROP called !!!");
        // Only cancel if this is the last clone
        if Arc::strong_count(&self.cancel) == 1 {
            eprintln!("!!! VmDevice: Last clone dropping, canceling dispatcher !!!");
            self.cancel.cancel(); // Cancel dispatcher
            self.transport_cancel.cancel(); // Cancel mux reader
        } else {
            eprintln!(
                "!!! VmDevice: Clone dropped, {} remaining !!!",
                Arc::strong_count(&self.cancel) - 1
            );
        }
    }
}

/// Upper bound on a single request/response round trip.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

impl VmDevice {
    pub fn from_transport(transport: PacketTransport, ctrl_if: Option<Interface>) -> Self {
        let (tx_only, mut incoming, transport_cancel) = transport.split();

        let response_channels = std::array::from_fn(|_| ResponseChannel::None);
        let state = Arc::new(State {
            response_channels: parking_lot::Mutex::new(response_channels),
            streams_active: StreamsActive::default(),
            next_generation: AtomicU64::new(1),
        });
        let thread_state = Arc::downgrade(&state);
        let state_cloned = Arc::clone(&state);

        use rand::Rng as _;
        let dispatcher_id: u32 = rand::thread_rng().r#gen();
        debug!(
            "Created new VmDevice State [Dispatcher ID:{}]",
            dispatcher_id
        );
        let dispatcher_id_task = dispatcher_id;

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_token_task = cancel_token.clone();
        tokio::spawn(async move {
            debug!("Dispatcher: [ID:{}] task started", dispatcher_id_task);
            use tokio_stream::StreamExt as _;
            loop {
                tokio::select! {
                    maybe_reply = incoming.next() => {
                        // Transport closed (USB reader died, mux unregistered): stop
                        // dispatching. Dropping the state closes every pending request
                        // so callers fail immediately instead of waiting for a reply
                        // that can no longer arrive.
                        let Some(reply) = maybe_reply else {
                            debug!("Dispatcher: [ID:{}] transport closed, exiting", dispatcher_id_task);
                            break;
                        };
                        debug!("Dispatcher: [ID:{}] received packet id={}", dispatcher_id_task, reply.id);
                        debug!("Dispatcher: [ID:{}] packet type = {:?}", dispatcher_id_task, std::mem::discriminant(&reply.data));
                        let mut chans = state_cloned.response_channels.lock();
                        let slot = &mut chans[usize::from(reply.id)];
                        debug!("Dispatcher: [ID:{}] slot state = {}", dispatcher_id_task, match slot {
                            ResponseChannel::None => "None",
                            ResponseChannel::Oneshot(..) => "Oneshot",
                            ResponseChannel::Stream(_) => "Stream",
                            ResponseChannel::Abandoned(_) => "Abandoned",
                        });
                        let data = reply.data;

                        let err = match std::mem::take(slot) {
                            ResponseChannel::None => {
                                warn!("Packet received for unregistered slot {}", reply.id);
                                None
                            }
                            ResponseChannel::Abandoned(_) => {
                                // Late reply to a request that timed out: its id was
                                // quarantined precisely so this cannot reach a newer one.
                                debug!("Dispatcher: [ID:{}] dropping late reply for abandoned slot {}", dispatcher_id_task, reply.id);
                                None
                            }
                            ResponseChannel::Oneshot(_, tx) => {
                                debug!("Dispatcher: [ID:{}] sending to oneshot channel", dispatcher_id_task);
                                let result = tx.send(data).err().map(|e| format!("{e:?}"));
                                debug!("Dispatcher: [ID:{}] oneshot send result = {:?}", dispatcher_id_task, if result.is_some() { "Error" } else { "Ok" });
                                result
                            }
                            ResponseChannel::Stream(tx) => {
                                let e = tx.try_send(data).err().map(|e| format!("{e:?}"));
                                *slot = ResponseChannel::Stream(tx);
                                e
                            }
                        };
                        if let Some(e) = err {
                            debug!("dispatcher failed to send reply: {e}");
                        } else {
                            debug!("Dispatcher: [ID:{}] successfully delivered packet id={}", dispatcher_id_task, reply.id);
                        }
                    }
                    () = cancel_token_task.cancelled() => {
                        debug!("Dispatcher: [ID:{}] cancelled, exiting", dispatcher_id_task);
                        break;
                    }
                }
            }
            debug!("Dispatcher: [ID:{}] task exits", dispatcher_id_task);
        });

        Self {
            transport: tx_only,
            thread_state,
            cancel: Arc::new(cancel_token),
            ctrl_if,
            transport_cancel,
        }
    }

    pub async fn connect_usb(info: DeviceInfo) -> Result<Self> {
        Self::connect_usb_inner(info, true).await
    }

    /// Connect over USB without requiring the device to be in USB data mode.
    /// Useful for control-only operations (e.g., pairing) while running in BLE mode.
    pub async fn connect_usb_any_mode(info: DeviceInfo) -> Result<Self> {
        Self::connect_usb_inner(info, false).await
    }

    async fn connect_usb_inner(info: DeviceInfo, require_usb_mode: bool) -> Result<Self> {
        let dev = info.open().await?;

        // Find the vendor-specific interface (class 0xFF)
        let vendor_iface_num = info
            .interfaces()
            .find(|iface| iface.class() == 0xFF)
            .map(nusb::InterfaceInfo::interface_number)
            .ok_or_else(|| anyhow!("No vendor interface (class 0xFF) found"))?;

        let iface = dev
            .claim_interface(vendor_iface_num)
            .await
            .map_err(|e| anyhow!("Failed to claim interface {vendor_iface_num}: {e}"))?;

        let in_ep = iface
            .endpoint::<Bulk, In>(0x81)
            .map_err(|e| anyhow!("Failed to get IN endpoint 0x81: {e}"))?
            .reader(4096);

        let out_ep = iface
            .endpoint::<Bulk, Out>(0x01)
            .map_err(|e| anyhow!("Failed to get OUT endpoint 0x01: {e}"))?
            .writer(4096);

        let transport = PacketTransport::usb(in_ep, out_ep);
        let ctrl_if = iface.clone();

        let device = Self::from_transport(transport, Some(ctrl_if));
        if require_usb_mode {
            let _ = device.clear_all_streams().await;
            let mode = device.get_transport_mode().await?;
            if !matches!(mode, TransportMode::Usb) {
                return Err(anyhow!("device transport mode is {mode:?}, expected Usb"));
            }
        }
        Ok(device)
    }
    pub async fn connect_via_mux(mux: MuxDevice, device_addr: [u8; 6]) -> Result<Self> {
        debug!(
            "VmDevice::connect_via_mux() creating new device for {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            device_addr[0],
            device_addr[1],
            device_addr[2],
            device_addr[3],
            device_addr[4],
            device_addr[5]
        );
        let transport = PacketTransport::mux(mux, device_addr);

        let device = Self::from_transport(transport, None);

        // Send DisableAll command and wait for Ack
        // This ensures any lingering streams from a previous session are stopped
        // before we proceed with settings loading
        debug!("Sending DisableAll to clear any active streams...");
        if let None = retry(
            async || {
                device
                    .request(PacketData::StreamUpdate(StreamUpdate {
                        packet_id: PacketType::End(),
                        action: crate::packets::vm::StreamUpdateAction::DisableAll,
                    }))
                    .await
            },
            Duration::from_millis(2000),
            5,
        )
        .await
        {
            return Err(anyhow!("Failed to disable all streams"));
        }
        debug!("Ready to proceed with settings loading (slot reservation prevents conflicts)");

        Ok(device)
    }

    pub async fn send(&self, pkt: Packet) -> Result<(), mpsc::error::SendError<Packet>> {
        self.transport.send(pkt).await
    }

    /// Send a request and wait for its reply. A device that never answers (e.g. it
    /// rebooted or switched transport mid-request) yields an error instead of
    /// hanging the caller forever.
    pub async fn request(&self, data: PacketData) -> Result<PacketData> {
        tokio::time::timeout(REQUEST_TIMEOUT, self.request_unbounded(data))
            .await
            .map_err(|_| anyhow!("request timed out after {REQUEST_TIMEOUT:?}"))?
    }

    async fn request_unbounded(&self, data: PacketData) -> Result<PacketData> {
        let (mut slot, recv) = self.get_oneshot_slot()?;
        self.send(Packet { id: slot.id, data }).await?;
        slot.sent = true;
        let r = recv.await?;
        slot.finished = true;
        Ok(r)
    }

    /// Like [`Self::request`], but re-sends when no reply arrives within `timeout`,
    /// up to `limit` attempts. Only a missing reply is retried; any other error
    /// (send failure, closed transport) is returned immediately. Only for
    /// idempotent requests: the first packet after a device (re)boots is sometimes
    /// lost.
    pub async fn request_with_retry(
        &self,
        data: PacketData,
        timeout: Duration,
        limit: usize,
    ) -> Result<PacketData> {
        retry(
            async || self.request_unbounded(data.clone()).await,
            timeout,
            limit,
        )
        .await
        .ok_or_else(|| anyhow!("no response after {limit} attempts of {timeout:?}"))?
    }

    fn get_oneshot_slot(&self) -> Result<(ResponseSlot, oneshot::Receiver<PacketData>)> {
        match self.thread_state.upgrade() {
            Some(thread_state) => {
                let mut response_channels = thread_state.response_channels.lock();
                // Slot reservation: IDs 0-207 for control, 208-255 for streaming
                // Control requests (oneshot) use the low range
                const CONTROL_ID_MAX: usize = 207;

                let now = tokio::time::Instant::now();
                let available = response_channels
                    .iter()
                    .take(CONTROL_ID_MAX + 1)
                    .filter(|c| c.is_free(now))
                    .count();
                debug!("get_oneshot_slot: {} slots available", available);
                for (i, c) in response_channels
                    .iter_mut()
                    .enumerate()
                    .take(CONTROL_ID_MAX + 1)
                {
                    if c.is_free(now) {
                        let (send, receiver) = oneshot::channel();
                        let generation =
                            thread_state.next_generation.fetch_add(1, Ordering::Relaxed);
                        *c = ResponseChannel::Oneshot(generation, send);
                        return Ok((
                            ResponseSlot {
                                thread_state: self.thread_state.clone(),
                                id: i as u8,
                                finished: false,
                                sent: false,
                                generation,
                            },
                            receiver,
                        ));
                    }
                }
            }
            _ => {
                debug!("ERROR: thread_state upgrade failed - State was dropped!");
            }
        }
        Err(anyhow::anyhow!("Failed to allocate request id"))
    }

    fn get_stream_slot(
        &self,
        buffer: usize,
    ) -> Result<(ResponseSlot, mpsc::Receiver<PacketData>), anyhow::Error> {
        if let Some(thread_state) = self.thread_state.upgrade() {
            let mut response_channels = thread_state.response_channels.lock();
            // Slot reservation: IDs 208-255 for streaming (48 slots), 0-207 for control
            // Streaming requests use the high range
            const STREAMING_ID_MIN: usize = 208;
            for (i, c) in response_channels
                .iter_mut()
                .enumerate()
                .skip(STREAMING_ID_MIN)
            {
                if let ResponseChannel::None = c {
                    let (send, receiver) = mpsc::channel(buffer);
                    *c = ResponseChannel::Stream(send);
                    return Ok((
                        ResponseSlot {
                            thread_state: self.thread_state.clone(),
                            id: i as u8,
                            finished: false,
                            sent: false,
                            generation: 0,
                        },
                        receiver,
                    ));
                }
            }
        }
        Err(anyhow::anyhow!("Failed to allocate request id"))
    }

    #[instrument(skip(self))]
    pub async fn read_register(&self, port: Port, bank: u8, address: u8) -> Result<u8> {
        let r = self
            .request(PacketData::ReadRegister(Register {
                port,
                bank,
                address,
            }))
            .await?
            .read_register_response()
            .with_context(|| "unexpected response")?;
        info!(data = r.data);
        assert_eq!(r.bank, bank);
        assert_eq!(r.address, address);
        Ok(r.data)
    }

    pub async fn write_register(&self, port: Port, bank: u8, address: u8, data: u8) -> Result<()> {
        info!("write_register: port={port:?} bank={bank} addr={address} data={data}");
        let data = PacketData::WriteRegister(WriteRegister {
            port,
            bank,
            address,
            data,
        });
        let pkt = Packet { id: 255, data };
        self.transport.writer.send(pkt).await?;
        Ok(())
    }

    pub async fn write_vendor(&self, tag: u8, data: &[u8]) -> Result<()> {
        assert!(tag > PacketType::VendorStart().into() && tag < PacketType::VendorEnd().into());
        let data_len = data.len();
        let data_padded: [u8; 98] = {
            let mut padded = [0; 98];
            padded[..data_len].copy_from_slice(data);
            padded
        };
        let data = PacketData::Vendor(
            tag,
            VendorData {
                len: data_len as u8,
                data: data_padded,
            },
        );
        let pkt = Packet { id: 255, data };
        self.transport.writer.send(pkt).await?;
        Ok(())
    }

    pub async fn read_config(&self, kind: ConfigKind) -> Result<GeneralConfig> {
        let r = self
            .request(PacketData::ReadConfig(kind))
            .await?
            .read_config_response()
            .with_context(|| "unexpected response")?;
        info!("config: {:?}", r);
        Ok(r)
    }

    pub async fn write_config(&self, config: GeneralConfig) -> Result<()> {
        info!("write_config: {config:?}");
        let data = PacketData::WriteConfig(config);
        let pkt = Packet { id: 255, data };
        self.transport.writer.send(pkt).await?;
        Ok(())
    }

    pub async fn read_prop(&self, kind: crate::packets::vm::PropKind) -> Result<Props> {
        let r = self
            .request(PacketData::ReadProp(kind))
            .await?
            .read_prop_response()
            .with_context(|| "unexpected response")?;
        info!("prop: {:?}", r);
        Ok(r)
    }

    /// Read all configuration values and return them as a single struct
    pub async fn read_all_config(&self) -> Result<GeneralSettings> {
        let impact_threshold = self.read_config(ConfigKind::ImpactThreshold).await?;
        let suppress_ms = self.read_config(ConfigKind::SuppressMs).await?;
        let accel_config = self.read_config(ConfigKind::AccelConfig).await?;
        let gyro_config = self.read_config(ConfigKind::GyroConfig).await?;
        let camera_model_nf = self.read_config(ConfigKind::CameraModelNf).await?;
        let camera_model_wf = self.read_config(ConfigKind::CameraModelWf).await?;
        let stereo_iso = self.read_config(ConfigKind::StereoIso).await?;

        Ok(GeneralSettings {
            impact_threshold: match impact_threshold {
                GeneralConfig::ImpactThreshold(val) => val,
                _ => anyhow::bail!("Unexpected config variant for ImpactThreshold"),
            },
            suppress_ms: match suppress_ms {
                GeneralConfig::SuppressMs(val) => val,
                _ => anyhow::bail!("Unexpected config variant for SuppressMs"),
            },
            accel_config: match accel_config {
                GeneralConfig::AccelConfig(val) => val,
                _ => anyhow::bail!("Unexpected config variant for AccelConfig"),
            },
            gyro_config: match gyro_config {
                GeneralConfig::GyroConfig(val) => val,
                _ => anyhow::bail!("Unexpected config variant for GyroConfig"),
            },
            camera_model_nf: match camera_model_nf {
                GeneralConfig::CameraModelNf(val) => RosOpenCvIntrinsics::try_from(val)
                    .map_err(|e| anyhow!("invalid camera model: {e}"))?,
                _ => anyhow::bail!("Unexpected config variant for CameraModelNf"),
            },
            camera_model_wf: match camera_model_wf {
                GeneralConfig::CameraModelWf(val) => RosOpenCvIntrinsics::try_from(val)
                    .map_err(|e| anyhow!("invalid camera model: {e}"))?,
                _ => anyhow::bail!("Unexpected config variant for CameraModelWf"),
            },
            stereo_iso: match stereo_iso {
                GeneralConfig::StereoIso(val) => val,
                _ => anyhow::bail!("Unexpected config variant for StereoIso"),
            },
        })
    }

    pub async fn get_frame(&self) -> Result<([MotData; 16], [MotData; 16])> {
        let r = self
            .request(PacketData::ObjectReportRequest())
            .await?
            .object_report()
            .with_context(|| "unexpected response")?;
        Ok((r.mot_data_nf, r.mot_data_wf))
    }

    pub async fn stream(
        &self,
        stream_type: PacketType,
    ) -> Result<impl Stream<Item = PacketData> + Send + Sync + use<>> {
        if let Some(thread_state) = self.thread_state.upgrade() {
            if thread_state.streams_active[stream_type].swap(true, Ordering::Relaxed) {
                return Err(anyhow!("cannot have more than one {stream_type:?} stream"));
            }
            let (slot, receiver) = self.get_stream_slot(100)?;
            trace!(
                "Requesting stream: type={:?}, slot_id={}",
                stream_type, slot.id
            );
            self.transport
                .writer
                .send(Packet {
                    id: slot.id,
                    data: PacketData::StreamUpdate(StreamUpdate {
                        packet_id: stream_type,
                        action: crate::packets::vm::StreamUpdateAction::Enable,
                    }),
                })
                .await?;
            return Ok(PacketStream {
                slot,
                receiver: ReceiverStream::new(receiver),
                sender: self.transport.writer.clone(),
                stream_type,
            });
        }
        Err(anyhow!("thread state dropped"))
    }

    pub async fn stream_mot_data(
        &self,
    ) -> Result<impl Stream<Item = ObjectReport> + Send + Sync + use<>> {
        Ok(self
            .stream(PacketType::ObjectReport())
            .await?
            .filter_map(PacketData::object_report))
    }

    pub async fn stream_combined_markers(
        &self,
    ) -> Result<impl Stream<Item = CombinedMarkersReport> + Send + Sync + use<>> {
        Ok(self
            .stream(PacketType::CombinedMarkersReport())
            .await?
            .filter_map(PacketData::combined_markers_report))
    }

    pub async fn stream_poc_markers(
        &self,
    ) -> Result<impl Stream<Item = PocMarkersReport> + Send + Sync + use<>> {
        Ok(self
            .stream(PacketType::PocMarkersReport())
            .await?
            .filter_map(PacketData::poc_markers_report))
    }

    pub async fn stream_accel(
        &self,
    ) -> Result<impl Stream<Item = AccelReport> + Send + Sync + use<>> {
        Ok(self
            .stream(PacketType::AccelReport())
            .await?
            .filter_map(PacketData::accel_report))
    }

    pub async fn stream_impact(
        &self,
    ) -> Result<impl Stream<Item = ImpactReport> + Send + Sync + use<>> {
        Ok(self
            .stream(PacketType::ImpactReport())
            .await?
            .filter_map(PacketData::impact_report))
    }

    pub async fn stream_battery(
        &self,
    ) -> Result<impl Stream<Item = BatteryReport> + Send + Sync + use<>> {
        Ok(self
            .stream(PacketType::BatteryReport())
            .await?
            .filter_map(PacketData::battery_report))
    }

    pub async fn flash_settings(&self) -> Result<()> {
        self.transport
            .writer
            .send(Packet {
                id: 255,
                data: PacketData::FlashSettings(),
            })
            .await?;
        Ok(())
    }

    pub async fn write_mode(&self, mode: odyssey_protocol::Mode) -> Result<()> {
        let data = PacketData::WriteMode(mode);
        let pkt = Packet { id: 255, data };
        self.transport.writer.send(pkt).await?;
        Ok(())
    }

    pub async fn clear_all_streams(&self) -> Result<()> {
        // Mark all streams as inactive to stop new packets
        if let Some(thread_state) = self.thread_state.upgrade() {
            for stream_type in 0u8..=4u8 {
                let packet_type = match stream_type {
                    0 => PacketType::ObjectReport(),
                    1 => PacketType::CombinedMarkersReport(),
                    2 => PacketType::PocMarkersReport(),
                    3 => PacketType::AccelReport(),
                    4 => PacketType::ImpactReport(),
                    _ => continue,
                };

                if thread_state.streams_active[packet_type].load(Ordering::Relaxed) {
                    info!("Clearing active stream: {:?}", packet_type);
                    thread_state.streams_active[packet_type].store(false, Ordering::Relaxed);
                }
            }
        }

        if let None = retry(
            async || {
                self.request(PacketData::StreamUpdate(StreamUpdate {
                    packet_id: PacketType::End(),
                    action: crate::packets::vm::StreamUpdateAction::DisableAll,
                }))
                .await
            },
            Duration::from_millis(2000),
            5,
        )
        .await
        {
            return Err(anyhow!("Failed to disable all streams"));
        }
        Ok(())
    }
}
