use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use nusb::{DeviceInfo, Interface};
use odyssey_protocol::control::device::TransportMode;
use odyssey_protocol::{PocMarkersReport, VendorData};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt as _};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, trace, warn};

use super::general_settings::GeneralSettings;
use super::hex_address::HexAddress;
use super::mux_device::MuxDevice;
use super::packet_stream::PacketStream;
use super::packet_transport::{PacketTransport, PacketTransportTx};
use super::response_slots::{Delivery, NO_REPLY_ID, ResponseSlot, State};
use super::retry::{STREAM_CONTROL_ATTEMPT_TIMEOUT, STREAM_CONTROL_ATTEMPTS, retry};
use super::usb_bulk_io::{find_vendor_interface, open_bulk_endpoints};
use crate::packets::vm::{
    AccelReport, BatteryReport, CombinedMarkersReport, ConfigKind, GeneralConfig, ImpactReport,
    MotData, ObjectReport, Packet, PacketData, PacketType, Port, PropKind, Props, Register,
    StreamUpdate, StreamUpdateAction, WriteRegister,
};

mod control_endpoint;
mod registers;
#[cfg(test)]
mod tests;

/// Upper bound on a single request/response round trip.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Replies a stream may have queued for its consumer before new ones are dropped.
const STREAM_REPLY_BUFFER: usize = 100;
/// Capacity of the payload of a vendor packet, the size of `VendorData::data`.
const VENDOR_PAYLOAD_CAPACITY: usize = 98;
/// Packet types a device streams, i.e. everything `clear_all_streams` resets.
const STREAM_PACKET_TYPES: [PacketType; 5] = [
    PacketType::ObjectReport(),
    PacketType::CombinedMarkersReport(),
    PacketType::PocMarkersReport(),
    PacketType::AccelReport(),
    PacketType::ImpactReport(),
];

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
    cancel: Arc<CancellationToken>, // Signals dispatcher to exit when VmDevice drops
    ctrl_if: Option<Interface>,
    transport_cancel: Arc<CancellationToken>, // Signals mux reader to exit when VmDevice drops
}

impl Drop for VmDevice {
    fn drop(&mut self) {
        // Only cancel if this is the last clone
        if Arc::strong_count(&self.cancel) == 1 {
            debug!("VmDevice: last clone dropping, cancelling dispatcher");
            self.cancel.cancel(); // Cancel dispatcher
            self.transport_cancel.cancel(); // Cancel mux reader
        } else {
            debug!(
                "VmDevice: clone dropped, {} remaining",
                Arc::strong_count(&self.cancel) - 1
            );
        }
    }
}

impl VmDevice {
    pub fn from_transport(transport: PacketTransport, ctrl_if: Option<Interface>) -> Self {
        let (tx_only, incoming, transport_cancel) = transport.split();

        let state = Arc::new(State::new());
        let thread_state = Arc::downgrade(&state);

        let cancel_token = CancellationToken::new();
        tokio::spawn(run_dispatcher(state, incoming, cancel_token.clone()));

        Self {
            transport: tx_only,
            thread_state,
            cancel: Arc::new(cancel_token),
            ctrl_if,
            transport_cancel,
        }
    }

    pub async fn connect_usb(info: DeviceInfo) -> Result<Self> {
        Self::connect_usb_inner(info, UsbModeRequirement::Required).await
    }

    /// Connect over USB without requiring the device to be in USB data mode.
    /// Useful for control-only operations (e.g., pairing) while running in BLE mode.
    pub async fn connect_usb_any_mode(info: DeviceInfo) -> Result<Self> {
        Self::connect_usb_inner(info, UsbModeRequirement::NotRequired).await
    }

    async fn connect_usb_inner(info: DeviceInfo, usb_mode: UsbModeRequirement) -> Result<Self> {
        let dev = info.open().await?;

        let vendor_iface_num = find_vendor_interface(&info)?;
        let iface = dev
            .claim_interface(vendor_iface_num)
            .await
            .map_err(|e| anyhow!("Failed to claim interface {vendor_iface_num}: {e}"))?;
        let (in_ep, out_ep) = open_bulk_endpoints(&iface)?;

        let transport = PacketTransport::usb(in_ep, out_ep);
        let device = Self::from_transport(transport, Some(iface));
        if usb_mode == UsbModeRequirement::Required {
            let _ = device.clear_all_streams().await;
            let mode = device.get_transport_mode().await?;
            if !matches!(mode, TransportMode::Usb) {
                bail!("device transport mode is {mode:?}, expected Usb");
            }
        }
        Ok(device)
    }

    pub async fn connect_via_mux(mux: MuxDevice, device_addr: [u8; 6]) -> Result<Self> {
        debug!(
            "VmDevice::connect_via_mux() creating new device for {}",
            HexAddress(device_addr)
        );
        let transport = PacketTransport::mux(mux, device_addr);

        let device = Self::from_transport(transport, None);

        // Stop any streams lingering from a previous session before the caller
        // starts loading settings.
        debug!("Sending DisableAll to clear any active streams...");
        device.disable_all_streams().await?;
        debug!("Ready to proceed with settings loading (slot reservation prevents conflicts)");

        Ok(device)
    }

    /// Asks the device to stop every stream, re-sending until it acknowledges.
    async fn disable_all_streams(&self) -> Result<()> {
        let acknowledged = retry(
            async || {
                self.request(PacketData::StreamUpdate(StreamUpdate {
                    packet_id: PacketType::End(),
                    action: StreamUpdateAction::DisableAll,
                }))
                .await
            },
            STREAM_CONTROL_ATTEMPT_TIMEOUT,
            STREAM_CONTROL_ATTEMPTS,
        )
        .await;
        match acknowledged {
            Some(_) => Ok(()),
            None => Err(anyhow!("Failed to disable all streams")),
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
            .await?;
        Ok(())
    }

    /// Send a request and wait for its reply. A device that never answers (e.g. it
    /// rebooted or switched transport mid-request) yields an error instead of
    /// hanging the caller forever.
    pub async fn request(&self, data: PacketData) -> Result<PacketData> {
        tokio::time::timeout(REQUEST_TIMEOUT, self.request_unbounded(data))
            .await
            .map_err(|_elapsed| anyhow!("request timed out after {REQUEST_TIMEOUT:?}"))?
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

    fn live_state(&self) -> Result<Arc<State>> {
        self.thread_state.upgrade().ok_or_else(|| {
            debug!("ERROR: thread_state upgrade failed - State was dropped!");
            anyhow!("Failed to allocate request id")
        })
    }

    fn get_oneshot_slot(&self) -> Result<(ResponseSlot, oneshot::Receiver<PacketData>)> {
        self.live_state()?
            .reserve_oneshot()
            .ok_or_else(|| anyhow!("Failed to allocate request id"))
    }

    fn get_stream_slot(&self, buffer: usize) -> Result<(ResponseSlot, mpsc::Receiver<PacketData>)> {
        self.live_state()?
            .reserve_stream(buffer)
            .ok_or_else(|| anyhow!("Failed to allocate request id"))
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
        if (r.bank, r.address) != (bank, address) {
            bail!(
                "register read answered for bank {} address {}, asked for bank {bank} address {address}",
                r.bank,
                r.address
            );
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
            bail!("vendor tag {tag:#04x} is outside the vendor range");
        }
        if data.len() > VENDOR_PAYLOAD_CAPACITY {
            bail!(
                "vendor payload of {} bytes exceeds the {VENDOR_PAYLOAD_CAPACITY}-byte capacity",
                data.len()
            );
        }
        let mut padded = [0; VENDOR_PAYLOAD_CAPACITY];
        padded[..data.len()].copy_from_slice(data);
        self.send_without_reply(PacketData::Vendor(
            tag,
            VendorData {
                // At most `VENDOR_PAYLOAD_CAPACITY`, checked above.
                len: data.len() as u8,
                data: padded,
            },
        ))
        .await
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
        self.send_without_reply(PacketData::WriteConfig(config))
            .await
    }

    pub async fn read_prop(&self, kind: PropKind) -> Result<Props> {
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
        GeneralSettings::read_with(|kind| self.read_config(kind)).await
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
        let Some(thread_state) = self.thread_state.upgrade() else {
            return Err(anyhow!("thread state dropped"));
        };
        if thread_state.streams_active[stream_type].swap(true, Ordering::Relaxed) {
            return Err(anyhow!("cannot have more than one {stream_type:?} stream"));
        }
        // A subscription that fails below must not leave the type marked busy.
        let release_stream_type = || {
            thread_state.streams_active[stream_type].store(false, Ordering::Relaxed);
        };
        let (slot, receiver) = match self.get_stream_slot(STREAM_REPLY_BUFFER) {
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
        if let Err(e) = self.transport.send(enable).await {
            release_stream_type();
            return Err(e.into());
        }
        Ok(PacketStream {
            slot,
            receiver: ReceiverStream::new(receiver),
            sender: self.transport.writer.clone(),
            stream_type,
        })
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

    pub async fn write_mode(&self, mode: odyssey_protocol::Mode) -> Result<()> {
        self.send_without_reply(PacketData::WriteMode(mode)).await
    }

    pub async fn clear_all_streams(&self) -> Result<()> {
        // Mark all streams as inactive to stop new packets
        if let Some(thread_state) = self.thread_state.upgrade() {
            for packet_type in STREAM_PACKET_TYPES {
                if thread_state.streams_active[packet_type].swap(false, Ordering::Relaxed) {
                    info!("Clearing active stream: {:?}", packet_type);
                }
            }
        }

        self.disable_all_streams().await
    }
}

/// Routes every packet the device sends to the request or stream waiting for
/// its id, until the transport closes or `cancel` fires.
///
/// When the transport closes, `state` is dropped on exit, which closes every
/// pending request so callers fail immediately instead of waiting for a reply
/// that can no longer arrive.
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
                        debug!("dispatcher failed to send reply: {why}");
                    }
                }
            }
            () = cancel.cancelled() => {
                debug!("Dispatcher: [ID:{dispatcher_id}] cancelled, exiting");
                break;
            }
        }
    }
    debug!("Dispatcher: [ID:{dispatcher_id}] task exits");
}
