use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

use backon::Retryable as _;
use nusb::{DeviceInfo, Interface};
use odyssey_protocol::control::device::TransportMode;
use odyssey_protocol::{PocMarkersReport, VendorData};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt as _};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, trace, warn};

use super::control_transfer::ControlLink;
use super::general_settings::GeneralSettings;
use super::hex_address::HexAddress;
use super::mux_device::MuxDevice;
use super::packet_stream::PacketStream;
use super::packet_transport::{PacketTransport, PacketTransportTx};
use super::response_slots::{Delivery, NO_REPLY_ID, ResponseSlot, State};
use super::retry::{STREAM_CONTROL_ATTEMPT_TIMEOUT, STREAM_CONTROL_ATTEMPTS, resend_schedule};
use super::usb_bulk_io::{find_vendor_interface, open_bulk_endpoints};
use crate::packets::vm::{
    AccelReport, BatteryReport, CombinedMarkersReport, ConfigKind, GeneralConfig, ImpactReport,
    MotData, ObjectReport, Packet, PacketData, PacketType, Port, PropKind, Props, Register,
    StreamUpdate, StreamUpdateAction, WriteRegister,
};
use crate::{Error, Result};

mod control_endpoint;
mod image_session;
mod registers;
#[cfg(test)]
mod tests;

/// Upper bound on a single request/response round trip.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Replies a stream may have queued for its consumer before new ones are dropped.
const STREAM_REPLY_BUFFER: usize = 100;

/// Whether a USB connection insists on the device being in USB data mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UsbModeRequirement {
    /// The device must report USB transport; anything else is an error.
    Required,
    /// Connect whatever the transport mode, e.g. to pair while in BLE mode.
    NotRequired,
}

#[derive(Clone, Debug)]
pub struct VmDevice {
    transport: PacketTransportTx,
    thread_state: Weak<State>,
    // Arc drops the guards exactly once even when clones drop concurrently.
    keepalive: Arc<(tokio_util::sync::DropGuard, tokio_util::sync::DropGuard)>,
    ctrl_if: Option<ControlLink<odyssey_protocol::control::device::DeviceMsg>>,
    // Serialize request/reply pairs; retain pairing events encountered while
    // waiting for a different reply.
    ctrl: Arc<tokio::sync::Mutex<Option<odyssey_protocol::control::device::PairingOutcome>>>,
    image_requests: mpsc::Sender<image_session::ImageRequest>,
    image_link: bool,
    image_mode_check: bool,
}

impl VmDevice {
    pub fn from_transport(transport: PacketTransport, ctrl_if: Option<Interface>) -> Self {
        Self::from_control_link(transport, ctrl_if.map(ControlLink::usb))
    }

    fn from_control_link(
        transport: PacketTransport,
        ctrl_if: Option<ControlLink<odyssey_protocol::control::device::DeviceMsg>>,
    ) -> Self {
        let image_link = matches!(
            transport.kind,
            super::packet_transport::LinkKind::Usb | super::packet_transport::LinkKind::Simulated
        );
        let image_mode_check =
            transport.kind == super::packet_transport::LinkKind::Usb && ctrl_if.is_some();
        let (tx_only, incoming, transport_cancel) = transport.split();

        let state = Arc::new(State::new());
        let thread_state = Arc::downgrade(&state);

        let cancel_token = CancellationToken::new();
        let dispatcher_transport_cancel = Arc::clone(&transport_cancel);
        let dispatcher_cancel = cancel_token.clone();
        let (image_requests, image_receiver) = mpsc::channel(1);
        let image_transport = tx_only.clone();
        let image_state = Weak::clone(&thread_state);
        let image_cancel = cancel_token.clone();
        let supervisor_cancel = cancel_token.clone();
        tokio::spawn(async move {
            // One supervised lifetime: image cleanup is awaited, never spawned
            // from Drop or detached from the owning transport dispatcher.
            tokio::join!(
                async {
                    run_dispatcher(state, incoming, dispatcher_cancel).await;
                    image_cancel.cancel();
                },
                image_session::supervise(
                    image_transport,
                    image_state,
                    image_receiver,
                    supervisor_cancel
                ),
            );
            dispatcher_transport_cancel.cancel();
        });

        Self {
            transport: tx_only,
            thread_state,
            keepalive: Arc::new((
                cancel_token.drop_guard(),
                transport_cancel.as_ref().clone().drop_guard(),
            )),
            ctrl_if,
            ctrl: Arc::new(tokio::sync::Mutex::new(None)),
            image_requests,
            image_link,
            image_mode_check,
        }
    }

    /// Opens an opt-in simulator link using bounded packet and control channels.
    /// The peer receives host requests on `outgoing` and `control_outgoing`.
    #[cfg(feature = "sim")]
    pub fn from_simulated_channels(
        outgoing: mpsc::Sender<Packet>,
        incoming: mpsc::Receiver<Packet>,
        control_outgoing: mpsc::Sender<odyssey_protocol::control::device::DeviceMsg>,
        control_incoming: mpsc::Receiver<odyssey_protocol::control::device::DeviceMsg>,
    ) -> Self {
        Self::from_control_link(
            PacketTransport::from_channels(outgoing, incoming),
            Some(ControlLink::simulated(control_outgoing, control_incoming)),
        )
    }

    pub async fn connect_usb(info: DeviceInfo) -> Result<Self> {
        Self::connect_usb_inner(info, UsbModeRequirement::Required).await
    }

    /// Connects whatever the transport mode, for control-endpoint operations
    /// such as pairing while the device runs in BLE mode.
    pub async fn connect_usb_any_mode(info: DeviceInfo) -> Result<Self> {
        Self::connect_usb_inner(info, UsbModeRequirement::NotRequired).await
    }

    async fn connect_usb_inner(info: DeviceInfo, usb_mode: UsbModeRequirement) -> Result<Self> {
        let dev = info.open().await?;

        let vendor_iface_num = find_vendor_interface(&info)?;
        let iface = dev.claim_interface(vendor_iface_num).await?;
        let (in_ep, out_ep) = open_bulk_endpoints(&iface)?;

        let transport = PacketTransport::usb(in_ep, out_ep);
        let device = Self::from_transport(transport, Some(iface));
        if usb_mode == UsbModeRequirement::Required {
            // Ask over the control endpoint first: firmware with a manual
            // transport mode (legacy vm-fw) answers bulk requests over BLE while
            // in BLE mode, so clearing streams first would spend every DisableAll
            // resend before reporting the wrong mode.
            let mode = device.get_transport_mode().await?;
            if mode != TransportMode::Usb {
                return Err(Error::WrongTransportMode {
                    expected: TransportMode::Usb,
                    actual: mode,
                });
            }
            if let Err(e) = device.clear_all_streams().await {
                warn!("could not clear streams left from a previous session: {e:#}");
            }
        }
        Ok(device)
    }

    pub async fn connect_via_mux(mux: MuxDevice, device_addr: [u8; 6]) -> Result<Self> {
        let device = Self::open_via_mux(mux, device_addr);

        // Stop any streams lingering from a previous session before the caller
        // starts loading settings.
        debug!("Sending DisableAll to clear any active streams...");
        device.disable_all_streams().await?;
        debug!("Ready to proceed with settings loading (slot reservation prevents conflicts)");

        Ok(device)
    }

    /// Opens a link through `mux` without touching the unit's streams.
    ///
    /// For a unit whose stream subscriptions are shared by all its links: a
    /// DisableAll sent while it streams over USB would stop those reports too.
    pub fn open_via_mux(mux: MuxDevice, device_addr: [u8; 6]) -> Self {
        debug!(
            "VmDevice::open_via_mux() creating new device for {}",
            HexAddress(device_addr)
        );
        Self::from_transport(PacketTransport::mux(mux, device_addr), None)
    }

    /// Asks the device to stop every stream, re-sending until it acknowledges.
    async fn disable_all_streams(&self) -> Result<()> {
        let acknowledged = (async || {
            tokio::time::timeout(
                STREAM_CONTROL_ATTEMPT_TIMEOUT,
                self.request(PacketData::StreamUpdate(StreamUpdate {
                    packet_id: PacketType::End(),
                    action: StreamUpdateAction::DisableAll,
                })),
            )
            .await
        })
        .retry(resend_schedule(STREAM_CONTROL_ATTEMPTS))
        .await
        .map_err(|_elapsed| Error::NoResponse {
            attempts: STREAM_CONTROL_ATTEMPTS,
            per_attempt: STREAM_CONTROL_ATTEMPT_TIMEOUT,
        })??;
        match acknowledged {
            PacketData::Ack() => Ok(()),
            other => Err(Error::unexpected(&other)),
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "SendError<Packet> hands the unsent packet back and is part of the public API"
    )]
    pub async fn send(&self, pkt: Packet) -> Result<(), mpsc::error::SendError<Packet>> {
        self.transport.send(pkt).await
    }

    /// Sends a packet that expects no reply.
    async fn send_without_reply(&self, data: PacketData) -> Result<()> {
        self.transport
            .send(Packet {
                id: NO_REPLY_ID,
                data,
            })
            .await
            .map_err(|_closed| Error::Closed)
    }

    /// Send a request and wait for its reply. A device that never answers (e.g. it
    /// rebooted or switched transport mid-request) yields an error instead of
    /// hanging the caller forever.
    pub async fn request(&self, data: PacketData) -> Result<PacketData> {
        tokio::time::timeout(REQUEST_TIMEOUT, self.request_unbounded(data))
            .await
            .map_err(|_elapsed| Error::Timeout(REQUEST_TIMEOUT))?
    }

    async fn request_unbounded(&self, data: PacketData) -> Result<PacketData> {
        let (mut slot, recv) = self.get_oneshot_slot()?;
        self.send(Packet { id: slot.id, data })
            .await
            .map_err(|_closed| Error::Closed)?;
        slot.sent = true;
        let reply = recv.await.map_err(|_dropped| Error::Closed)?;
        slot.finished = true;
        Ok(reply)
    }

    /// Like [`Self::request`], but re-sends when no reply arrives within `timeout`,
    /// up to `limit` attempts. Only a missing reply is retried; any other error
    /// (send failure, closed transport) is returned immediately. Only for
    /// idempotent requests: the first packet after a device (re)boots is sometimes
    /// lost. A zero limit sends nothing and returns [`Error::NoResponse`].
    pub async fn request_with_retry(
        &self,
        data: PacketData,
        timeout: Duration,
        limit: usize,
    ) -> Result<PacketData> {
        if limit == 0 {
            return Err(Error::NoResponse {
                attempts: 0,
                per_attempt: timeout,
            });
        }
        (async || tokio::time::timeout(timeout, self.request_unbounded(data.clone())).await)
            .retry(resend_schedule(limit))
            .await
            .map_err(|_elapsed| Error::NoResponse {
                attempts: limit,
                per_attempt: timeout,
            })?
    }

    /// The shared request table, or [`Error::Closed`] once the dispatcher that
    /// owns it has exited.
    fn live_state(&self) -> Result<Arc<State>> {
        self.thread_state.upgrade().ok_or(Error::Closed)
    }

    fn get_oneshot_slot(&self) -> Result<(ResponseSlot, oneshot::Receiver<PacketData>)> {
        self.live_state()?.reserve_oneshot()
    }

    fn get_stream_slot(
        &self,
        buffer: usize,
        stream_type: PacketType,
    ) -> Result<(ResponseSlot, mpsc::Receiver<PacketData>)> {
        self.live_state()?.reserve_stream(buffer, stream_type)
    }

    #[instrument(skip(self))]
    pub async fn read_register(&self, port: Port, bank: u8, address: u8) -> Result<u8> {
        let reply = self
            .request(PacketData::ReadRegister(Register {
                port,
                bank,
                address,
            }))
            .await?;
        let PacketData::ReadRegisterResponse(r) = reply else {
            return Err(Error::unexpected(&reply));
        };
        info!(data = r.data);
        if (r.bank, r.address) != (bank, address) {
            return Err(Error::unexpected(&r));
        }
        Ok(r.data)
    }

    pub async fn write_register(&self, port: Port, bank: u8, address: u8, data: u8) -> Result<()> {
        info!("write_register: port={port:?} bank={bank} addr={address} data={data}");
        self.send_without_reply(PacketData::WriteRegister(WriteRegister {
            port,
            bank,
            address,
            data,
        }))
        .await
    }

    pub async fn write_vendor(&self, tag: u8, data: &[u8]) -> Result<()> {
        // Both ends of the range are marker ids, not tags a payload may carry.
        let first_tag = u8::from(PacketType::VendorStart()) + 1;
        if !(first_tag..u8::from(PacketType::VendorEnd())).contains(&tag) {
            return Err(Error::InvalidArgument(
                "vendor tag outside the vendor range",
            ));
        }
        if data.len() > VendorData::CAPACITY {
            return Err(Error::InvalidArgument(
                "vendor payload larger than VendorData::CAPACITY",
            ));
        }
        let mut padded = [0; VendorData::CAPACITY];
        padded[..data.len()].copy_from_slice(data);
        self.send_without_reply(PacketData::Vendor(
            tag,
            VendorData {
                // At most `VendorData::CAPACITY`, checked above.
                len: data.len() as u8,
                data: padded,
            },
        ))
        .await
    }

    pub async fn read_config(&self, kind: ConfigKind) -> Result<GeneralConfig> {
        let reply = self.request(PacketData::ReadConfig(kind)).await?;
        let PacketData::ReadConfigResponse(r) = reply else {
            return Err(Error::unexpected(&reply));
        };
        info!("config: {:?}", r);
        Ok(r)
    }

    pub async fn write_config(&self, config: GeneralConfig) -> Result<()> {
        info!("write_config: {config:?}");
        self.send_without_reply(PacketData::WriteConfig(config))
            .await
    }

    pub async fn read_prop(&self, kind: PropKind) -> Result<Props> {
        let reply = self.request(PacketData::ReadProp(kind)).await?;
        let PacketData::ReadPropResponse(r) = reply else {
            return Err(Error::unexpected(&reply));
        };
        info!("prop: {:?}", r);
        Ok(r)
    }

    pub async fn read_all_config(&self) -> Result<GeneralSettings> {
        GeneralSettings::read_with(|kind| self.read_config(kind)).await
    }

    pub async fn get_frame(&self) -> Result<([MotData; 16], [MotData; 16])> {
        let reply = self.request(PacketData::ObjectReportRequest()).await?;
        let PacketData::ObjectReport(r) = reply else {
            return Err(Error::unexpected(&reply));
        };
        Ok((r.mot_data_nf, r.mot_data_wf))
    }

    /// Starts one USB-only image session. The owned stream is Send + 'static,
    /// cancels on drop and keeps only the most recent complete frame. Its
    /// supervised stop handshake completes before a replacement can start.
    pub async fn stream_images(
        &self,
        config: odyssey_protocol::image::ImageStreamConfig,
    ) -> Result<crate::image::ImageStream> {
        if !self.image_link {
            return Err(Error::ImageUsbRequired);
        }
        if !(1..=15).contains(&config.fps) {
            return Err(Error::InvalidArgument("image FPS must be in 1..=15"));
        }
        // A direct handle opened for pairing may still have BLE data selected.
        // Check the control endpoint before issuing any image data command.
        if self.image_mode_check && self.get_transport_mode().await? != TransportMode::Usb {
            return Err(Error::ImageUsbRequired);
        }
        let cancel = CancellationToken::new();
        let guard = cancel.clone().drop_guard();
        let (ready, response) = oneshot::channel();
        self.image_requests
            .try_send(image_session::ImageRequest {
                config,
                cancel,
                ready,
                _keepalive: Arc::clone(&self.keepalive),
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => {
                    Error::ImageStatus(odyssey_protocol::image::ImageStatusCode::Busy)
                }
                mpsc::error::TrySendError::Closed(_) => Error::Closed,
            })?;
        let result = response.await.map_err(|_closed| Error::Closed)?;
        guard.disarm();
        result
    }

    pub async fn stream(
        &self,
        stream_type: PacketType,
    ) -> Result<impl Stream<Item = PacketData> + Send + Sync + use<>> {
        let thread_state = self.live_state()?;
        let _control = thread_state.stream_control.lock().await;
        if thread_state.streams_active[stream_type].swap(true, Ordering::Relaxed) {
            return Err(Error::StreamOpen(stream_type));
        }
        // A subscription that fails below must not leave the type marked busy.
        let release_stream_type = || {
            thread_state.streams_active[stream_type].store(false, Ordering::Relaxed);
        };
        let (slot, receiver) = match self.get_stream_slot(STREAM_REPLY_BUFFER, stream_type) {
            Ok(reserved) => reserved,
            Err(e) => {
                release_stream_type();
                return Err(e);
            }
        };
        trace!(
            "Requesting stream: type={:?}, slot_id={}",
            stream_type, slot.id
        );
        let enable = Packet {
            id: slot.id,
            data: PacketData::StreamUpdate(StreamUpdate {
                packet_id: stream_type,
                action: StreamUpdateAction::Enable,
            }),
        };
        // Construct the guard before the cancellable send: dropping this
        // future must release both its slot and the stream-type reservation.
        let stream = PacketStream {
            slot: Some(slot),
            receiver: ReceiverStream::new(receiver),
            sender: self.transport.writer.clone(),
            stream_type,
        };
        self.transport
            .send(enable)
            .await
            .map_err(|_closed| Error::Closed)?;
        Ok(stream)
    }

    /// A stream narrowed to the one report type `extract` picks out.
    async fn stream_of<T>(
        &self,
        stream_type: PacketType,
        extract: fn(PacketData) -> Option<T>,
    ) -> Result<impl Stream<Item = T> + Send + Sync + use<T>> {
        Ok(self.stream(stream_type).await?.filter_map(extract))
    }

    pub async fn stream_mot_data(
        &self,
    ) -> Result<impl Stream<Item = ObjectReport> + Send + Sync + use<>> {
        self.stream_of(PacketType::ObjectReport(), PacketData::object_report)
            .await
    }

    pub async fn stream_combined_markers(
        &self,
    ) -> Result<impl Stream<Item = CombinedMarkersReport> + Send + Sync + use<>> {
        self.stream_of(
            PacketType::CombinedMarkersReport(),
            PacketData::combined_markers_report,
        )
        .await
    }

    pub async fn stream_poc_markers(
        &self,
    ) -> Result<impl Stream<Item = PocMarkersReport> + Send + Sync + use<>> {
        self.stream_of(
            PacketType::PocMarkersReport(),
            PacketData::poc_markers_report,
        )
        .await
    }

    pub async fn stream_accel(
        &self,
    ) -> Result<impl Stream<Item = AccelReport> + Send + Sync + use<>> {
        self.stream_of(PacketType::AccelReport(), PacketData::accel_report)
            .await
    }

    pub async fn stream_impact(
        &self,
    ) -> Result<impl Stream<Item = ImpactReport> + Send + Sync + use<>> {
        self.stream_of(PacketType::ImpactReport(), PacketData::impact_report)
            .await
    }

    pub async fn stream_battery(
        &self,
    ) -> Result<impl Stream<Item = BatteryReport> + Send + Sync + use<>> {
        self.stream_of(PacketType::BatteryReport(), PacketData::battery_report)
            .await
    }

    pub async fn flash_settings(&self) -> Result<()> {
        self.send_without_reply(PacketData::FlashSettings()).await
    }

    /// Stops firmware streaming and closes existing subscriptions. Generation
    /// checks keep old subscription guards from disabling their replacements.
    pub async fn clear_all_streams(&self) -> Result<()> {
        let state = self.live_state()?;
        let _control = state.stream_control.lock().await;
        self.disable_all_streams().await?;
        state.clear_streams();
        Ok(())
    }

    /// Closes this link's subscriptions without telling the device, so dropping
    /// them sends no Disable. For a device whose subscriptions are shared with
    /// another link that keeps streaming: a Disable over this link would stop
    /// that link's reports too.
    pub fn forget_streams(&self) {
        if let Some(state) = self.thread_state.upgrade() {
            state.clear_streams();
        }
    }
}

/// Routes every packet the device sends to the request or stream waiting for
/// its id, until the transport closes or `cancel` fires.
///
/// On exit, explicitly close every response channel, even if a setup/reset
/// operation still holds `state`. Pending requests fail immediately and no new
/// request can reserve an id on the dead link.
async fn run_dispatcher(
    state: Arc<State>,
    mut incoming: ReceiverStream<Packet>,
    cancel: CancellationToken,
) {
    use rand::Rng as _;
    let dispatcher_id: u32 = rand::thread_rng().r#gen();
    debug!("Dispatcher: [ID:{dispatcher_id}] task started");
    loop {
        tokio::select! {
            maybe_reply = incoming.next() => {
                // Transport closed (USB reader died, mux unregistered): stop
                // dispatching.
                let Some(reply) = maybe_reply else {
                    debug!("Dispatcher: [ID:{dispatcher_id}] transport closed, exiting");
                    break;
                };
                if let Some(sender) = state.image_sender(&reply) {
                    // Reassembly drains independently of the UI. Backpressure
                    // this bounded queue instead of corrupting a pixel frame by
                    // dropping one arbitrary chunk during a USB burst.
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        _ = sender.send(reply.data) => {},
                    }
                    continue;
                }
                let id = reply.id;
                match state.deliver(reply) {
                    Delivery::Delivered => {
                        debug!("Dispatcher: [ID:{dispatcher_id}] delivered packet id={id}");
                    }
                    Delivery::Unregistered => {
                        warn!("Packet received for unregistered slot {id}");
                    }
                    Delivery::LateReplyDropped => {
                        debug!(
                            "Dispatcher: [ID:{dispatcher_id}] dropping late reply for abandoned slot {id}"
                        );
                    }
                    Delivery::Undeliverable(why) => {
                        debug!("Dispatcher: [ID:{dispatcher_id}] dropped reply id={id}: {why:?}");
                    }
                }
            }
            () = cancel.cancelled() => {
                debug!("Dispatcher: [ID:{dispatcher_id}] cancelled, exiting");
                break;
            }
        }
    }
    state.close();
    debug!("Dispatcher: [ID:{dispatcher_id}] task exits");
}
