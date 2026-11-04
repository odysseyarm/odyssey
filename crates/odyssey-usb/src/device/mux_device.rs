use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use heapless::Vec as HVec;
use nusb::{DeviceInfo, Interface};
use odyssey_protocol::control::usb_mux::{
    ClearBondsError, ClearBondsResult, PairingError, PairingOutcome, StartPairing, UsbMuxCtrlMsg,
};
use odyssey_protocol::control::{AddBondResult, BondEntry, BondedDevice};
use tokio::sync::mpsc;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, warn};

use super::control_transfer::{self, FLASH_WRITE_REPLY_TIMEOUT, REPLY_TIMEOUT, UndecodableReply};
use super::mux_transport::{DevicePacketRoutes, DeviceSnapshot, MuxTransport, MuxTransportParts};
use super::usb_bulk_io::open_bulk_endpoints;
use crate::packets::mux::{MAX_DEVICES, MuxMsg, SendTo};
use crate::packets::vm::Packet;

/// A dongle exposes its bulk endpoints on one of its first interfaces; probing
/// stops after this many.
const MAX_PROBED_INTERFACES: u8 = 8;
/// How many times `request_devices` asks before giving up.
const REQUEST_DEVICES_ATTEMPTS: u32 = 3;
/// Pause between `request_devices` attempts.
const REQUEST_DEVICES_BACKOFF: Duration = Duration::from_millis(200);
/// How long `request_devices` waits for one snapshot.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(1);
/// A stale snapshot is only drained if it is already queued; this is the most
/// time spent checking for one.
const STALE_SNAPSHOT_POLL: Duration = Duration::from_millis(10);
/// Longest a pairing session can run before the dongle reports a timeout.
const PAIRING_EVENT_TIMEOUT: Duration = Duration::from_secs(120);
/// The dongle clears bonds synchronously and may drop the response when busy;
/// after this long without one, the clear is assumed to have worked.
const CLEAR_BONDS_PATIENCE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct MuxDevice {
    writer: mpsc::Sender<MuxMsg>,
    snapshots_rx: Arc<tokio::sync::Mutex<ReceiverStream<DeviceSnapshot>>>,
    msg_rx: Arc<tokio::sync::Mutex<ReceiverStream<MuxMsg>>>,
    pub(super) device_packets_tx: Arc<DevicePacketRoutes>,
    ctrl_if: Interface,
}

#[derive(Debug, Clone)]
pub enum PairingEvent {
    Result(BondedDevice),
    Timeout,
    Cancelled,
}

impl MuxDevice {
    async fn ctrl_recv_filtered(
        &self,
        timeout: Duration,
        accept: impl FnMut(&UsbMuxCtrlMsg) -> bool,
    ) -> Result<UsbMuxCtrlMsg> {
        control_transfer::recv_matching(&self.ctrl_if, timeout, UndecodableReply::Skip, accept)
            .await
    }

    async fn ctrl_send(&self, msg: UsbMuxCtrlMsg) -> Result<()> {
        control_transfer::send(&self.ctrl_if, &msg).await
    }

    pub fn from_transport(transport: MuxTransport, ctrl_if: Interface) -> Self {
        let MuxTransportParts {
            writer,
            snapshots,
            control,
            device_packets_tx,
        } = transport.split();

        Self {
            writer,
            snapshots_rx: Arc::new(tokio::sync::Mutex::new(snapshots)),
            msg_rx: Arc::new(tokio::sync::Mutex::new(control)),
            device_packets_tx,
            ctrl_if,
        }
    }

    pub async fn connect_usb(info: DeviceInfo) -> Result<Self> {
        let dev = info.open().await?;

        // Probe interfaces to find the one that has our bulk endpoints.
        for idx in 0..MAX_PROBED_INTERFACES {
            let Ok(iface) = dev.claim_interface(idx).await else {
                continue;
            };
            let Ok((in_ep, out_ep)) = open_bulk_endpoints(&iface) else {
                continue;
            };

            let transport = MuxTransport::usb(in_ep, out_ep);
            return Ok(Self::from_transport(transport, iface));
        }
        Err(anyhow!(
            "failed to find mux interface with required endpoints"
        ))
    }

    async fn send_msg(&self, msg: MuxMsg) -> Result<()> {
        self.writer
            .send(msg)
            .await
            .map_err(|e| anyhow!("send failed: {e}"))?;
        Ok(())
    }

    pub async fn receive_msg(&self) -> Result<MuxMsg> {
        let mut rx = self.msg_rx.lock().await;
        let msg = rx.next().await.ok_or_else(|| anyhow!("channel closed"))?;
        debug!("MuxDevice::receive_msg got control message");
        Ok(msg)
    }

    pub async fn receive_snapshot(&self) -> Result<DeviceSnapshot> {
        let mut rx = self.snapshots_rx.lock().await;
        let devices = rx.next().await.ok_or_else(|| anyhow!("channel closed"))?;
        debug!("MuxDevice::receive_snapshot got device snapshot");
        Ok(devices)
    }

    /// Drops snapshots that were queued before the caller asked, so the next
    /// one received answers the new request.
    async fn drain_stale_snapshots(&self) {
        let mut rx = self.snapshots_rx.lock().await;
        while let Ok(Some(_)) = tokio::time::timeout(STALE_SNAPSHOT_POLL, rx.next()).await {
            debug!("Drained stale snapshot");
        }
    }

    pub async fn request_devices(&self) -> Result<DeviceSnapshot> {
        let mut last_err = None;
        for attempt in 0..REQUEST_DEVICES_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(REQUEST_DEVICES_BACKOFF).await;
            }

            self.drain_stale_snapshots().await;

            if let Err(e) = self.send_msg(MuxMsg::RequestDevices()).await {
                last_err = Some(e);
                continue;
            }

            match tokio::time::timeout(SNAPSHOT_TIMEOUT, self.receive_snapshot()).await {
                Ok(Ok(devices)) => return Ok(devices),
                Ok(Err(e)) => last_err = Some(e),
                Err(_elapsed) => last_err = Some(anyhow!("timeout waiting for DevicesSnapshot")),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("request_devices failed")))
    }

    /// Subscribe to device list changes. After calling this, the dongle will
    /// automatically send DevicesSnapshot messages whenever devices connect or disconnect.
    /// You should have a task calling receive_msg() to process these notifications.
    pub async fn subscribe_device_list(&self) -> Result<()> {
        self.send_msg(MuxMsg::SubscribeDeviceList()).await
    }

    /// Unsubscribe from device list changes.
    pub async fn unsubscribe_device_list(&self) -> Result<()> {
        self.send_msg(MuxMsg::UnsubscribeDeviceList()).await
    }

    pub async fn send_to(&self, dev: [u8; 6], pkt: Packet) -> Result<()> {
        let msg = MuxMsg::SendTo(SendTo { dev, pkt });
        self.send_msg(msg).await
    }

    pub async fn read_version(&self) -> Result<crate::packets::mux::Version> {
        self.send_msg(MuxMsg::ReadVersion()).await?;

        match self.receive_msg().await? {
            MuxMsg::ReadVersionResponse(version) => Ok(version),
            other => Err(anyhow!("unexpected response: {other:?}")),
        }
    }

    pub async fn read_ctrl_version(&self) -> Result<odyssey_protocol::Version> {
        self.ctrl_send(UsbMuxCtrlMsg::ReadVersion()).await?;
        match self
            .ctrl_recv_filtered(REPLY_TIMEOUT, |msg| {
                matches!(msg, UsbMuxCtrlMsg::ReadVersionResponse(_))
            })
            .await?
        {
            UsbMuxCtrlMsg::ReadVersionResponse(version) => Ok(version),
            other => Err(anyhow!("unexpected ctrl response: {other:?}")),
        }
    }

    /// Start pairing mode on the dongle with a timeout (ms).
    pub async fn start_pairing(&self, timeout_ms: u32) -> Result<()> {
        self.ctrl_send(UsbMuxCtrlMsg::StartPairing(StartPairing { timeout_ms }))
            .await?;
        match self
            .ctrl_recv_filtered(REPLY_TIMEOUT, |msg| {
                matches!(
                    msg,
                    UsbMuxCtrlMsg::StartPairingResponse() | UsbMuxCtrlMsg::PairingResult(_)
                )
            })
            .await?
        {
            UsbMuxCtrlMsg::StartPairingResponse() => Ok(()),
            // Pairing finished immediately; treat as success so the caller can proceed
            UsbMuxCtrlMsg::PairingResult(PairingOutcome::Ok(_addr)) => Ok(()),
            other => Err(anyhow!("unexpected ctrl response: {other:?}")),
        }
    }

    /// Cancel pairing mode on the dongle.
    pub async fn cancel_pairing(&self) -> Result<()> {
        self.ctrl_send(UsbMuxCtrlMsg::CancelPairing()).await
    }

    /// Wait for a pairing event from the dongle.
    pub async fn wait_pairing_event(&self) -> Result<PairingEvent> {
        loop {
            match self
                .ctrl_recv_filtered(PAIRING_EVENT_TIMEOUT, |msg| {
                    matches!(msg, UsbMuxCtrlMsg::PairingResult(_))
                })
                .await?
            {
                UsbMuxCtrlMsg::PairingResult(PairingOutcome::Ok(device)) => {
                    return Ok(PairingEvent::Result(device));
                }
                UsbMuxCtrlMsg::PairingResult(PairingOutcome::Err(PairingError::Timeout)) => {
                    return Ok(PairingEvent::Timeout);
                }
                UsbMuxCtrlMsg::PairingResult(PairingOutcome::Err(PairingError::Cancelled)) => {
                    return Ok(PairingEvent::Cancelled);
                }
                _ => { /* filtered */ }
            }
        }
    }

    pub async fn clear_bonds(&self) -> Result<()> {
        self.ctrl_send(UsbMuxCtrlMsg::ClearBonds()).await?;
        let start = Instant::now();
        loop {
            match self
                .ctrl_recv_filtered(REPLY_TIMEOUT, |msg| {
                    matches!(
                        msg,
                        UsbMuxCtrlMsg::ClearBondsResponse(_) | UsbMuxCtrlMsg::PairingResult(_)
                    )
                })
                .await
            {
                Ok(msg) => match msg {
                    UsbMuxCtrlMsg::ClearBondsResponse(ClearBondsResult::Ok()) => return Ok(()),
                    UsbMuxCtrlMsg::ClearBondsResponse(ClearBondsResult::Err(
                        ClearBondsError::Failed,
                    )) => return Err(anyhow!("Failed to clear bonds")),
                    // Ignore unrelated ctrl messages (e.g., pairing result racing with clear)
                    _ if start.elapsed() < CLEAR_BONDS_PATIENCE => {}
                    _ => {
                        warn!("no ClearBonds response received; assuming success");
                        return Ok(());
                    }
                },
                // Treat timeouts as benign while waiting; after the patience runs out, assume success.
                Err(e) if start.elapsed() < CLEAR_BONDS_PATIENCE => {
                    debug!("ClearBonds: still waiting for a response: {e}");
                }
                Err(e) => {
                    warn!("ClearBonds timeout waiting for ctrl event: {e}; assuming success");
                    return Ok(());
                }
            }
        }
    }

    pub async fn list_bonds(&self) -> Result<HVec<BondedDevice, MAX_DEVICES>> {
        self.ctrl_send(UsbMuxCtrlMsg::ListBonds()).await?;
        match self
            .ctrl_recv_filtered(REPLY_TIMEOUT, |msg| {
                matches!(msg, UsbMuxCtrlMsg::ListBondsResponse(_))
            })
            .await?
        {
            UsbMuxCtrlMsg::ListBondsResponse(bonds) => Ok(bonds.0),
            other => Err(anyhow!("unexpected ctrl response: {other:?}")),
        }
    }

    pub async fn update_bond_name(&self, uuid: [u8; 6], name: heapless::String<32>) -> Result<()> {
        self.ctrl_send(UsbMuxCtrlMsg::UpdateBondName {
            uuid,
            name: name.into(),
        })
        .await
    }

    pub async fn add_bond(&self, entry: BondEntry) -> Result<()> {
        self.ctrl_send(UsbMuxCtrlMsg::AddBond(entry)).await?;
        match self
            .ctrl_recv_filtered(FLASH_WRITE_REPLY_TIMEOUT, |msg| {
                matches!(msg, UsbMuxCtrlMsg::AddBondResponse(_))
            })
            .await?
        {
            UsbMuxCtrlMsg::AddBondResponse(AddBondResult::Ok()) => Ok(()),
            UsbMuxCtrlMsg::AddBondResponse(AddBondResult::Err(e)) => {
                Err(anyhow!("AddBond failed: {e:?}"))
            }
            other => Err(anyhow!("unexpected ctrl response: {other:?}")),
        }
    }
}
