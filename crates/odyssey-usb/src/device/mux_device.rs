use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use heapless::Vec as HVec;
use nusb::transfer::{Bulk, ControlIn, ControlOut, ControlType, In, Out, Recipient, TransferError};
use nusb::{DeviceInfo, Interface};
use odyssey_protocol::control::usb_mux::{ClearBondsError, PairingError, UsbMuxCtrlMsg};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::debug;

use super::mux_transport::MuxTransport;
use crate::packets::mux::{MAX_DEVICES, MuxMsg, SendTo};
use crate::packets::vm::Packet;

#[derive(Clone, Debug)]
pub struct MuxDevice {
    writer: mpsc::Sender<MuxMsg>,
    snapshots_rx: Arc<tokio::sync::Mutex<ReceiverStream<HVec<[u8; 6], MAX_DEVICES>>>>,
    msg_rx: Arc<tokio::sync::Mutex<ReceiverStream<MuxMsg>>>,
    pub(super) device_packets_tx: Arc<Mutex<HashMap<[u8; 6], mpsc::Sender<Packet>>>>,
    ctrl_if: Interface,
}

impl MuxDevice {
    #[expect(dead_code)]
    async fn ctrl_recv_polling(&self, timeout: Duration) -> Result<UsbMuxCtrlMsg> {
        let idx = u16::from(self.ctrl_if.interface_number());
        let start = std::time::Instant::now();
        loop {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return Err(anyhow!("timeout waiting for ctrl event"));
            }
            let slice = (timeout - elapsed).min(Duration::from_millis(500));
            match self
                .ctrl_if
                .control_in(
                    ControlIn {
                        control_type: ControlType::Vendor,
                        recipient: Recipient::Interface,
                        request: Self::USB_MUX_CTRL_REQ_RECV,
                        value: 0,
                        index: idx,
                        length: 256,
                    },
                    slice,
                )
                .await
            {
                Ok(data) => {
                    if data.is_empty() {
                        continue;
                    }
                    break postcard::from_bytes::<UsbMuxCtrlMsg>(&data)
                        .map_err(|e| anyhow!("ctrl decode failed: {e}"));
                }
                Err(_) => {
                    continue;
                }
            }
        }
    }

    async fn ctrl_recv_filtered<F>(&self, timeout: Duration, mut accept: F) -> Result<UsbMuxCtrlMsg>
    where
        F: FnMut(&UsbMuxCtrlMsg) -> bool,
    {
        let idx = u16::from(self.ctrl_if.interface_number());
        let start = std::time::Instant::now();
        loop {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return Err(anyhow!("timeout waiting for ctrl event"));
            }
            let slice = (timeout - elapsed).min(Duration::from_millis(500));
            let reply = match self
                .ctrl_if
                .control_in(
                    ControlIn {
                        control_type: ControlType::Vendor,
                        recipient: Recipient::Interface,
                        request: Self::USB_MUX_CTRL_REQ_RECV,
                        value: 0,
                        index: idx,
                        length: 256,
                    },
                    slice,
                )
                .await
            {
                Ok(data) => data,
                Err(TransferError::Cancelled) => continue, // poll-slice timeout, retry
                Err(e) => return Err(anyhow!("ctrl_recv control_in failed: {e:?}")),
            };

            if reply.is_empty() {
                continue;
            }

            let msg = match postcard::from_bytes::<UsbMuxCtrlMsg>(&reply) {
                Ok(msg) => msg,
                Err(e) => {
                    if start.elapsed() < timeout {
                        continue;
                    }
                    return Err(anyhow!("ctrl decode failed: {e}"));
                }
            };

            if accept(&msg) {
                return Ok(msg);
            }
        }
    } // USB Mux control over EP0 (Interface recipient)
    const USB_MUX_CTRL_REQ_SEND: u8 = 0x30; // vendor request to send ctrl msg
    const USB_MUX_CTRL_REQ_RECV: u8 = 0x31; // vendor request to receive ctrl msg

    async fn ctrl_send(&self, msg: UsbMuxCtrlMsg) -> Result<()> {
        use std::time::Duration;
        let data = postcard::to_allocvec(&msg).map_err(|e| anyhow!("ctrl encode failed: {e}"))?;
        self.ctrl_if
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Interface,
                    request: Self::USB_MUX_CTRL_REQ_SEND,
                    value: 0,
                    index: u16::from(self.ctrl_if.interface_number()),
                    data: &data,
                },
                Duration::from_millis(1000),
            )
            .await
            .map_err(|e| anyhow!("control_out failed: {e}"))?;
        Ok(())
    }

    pub fn from_transport(transport: MuxTransport, ctrl_if: Interface) -> Self {
        let (writer, snapshots, control, device_packets_tx) = transport.split();

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

        // Probe interfaces to find the one that has our bulk endpoints (0x01 OUT, 0x81 IN).
        for idx in 0u8..8 {
            let iface = match dev.claim_interface(idx).await {
                Ok(i) => i,
                Err(_) => continue,
            };

            let in_ep = match iface.endpoint::<Bulk, In>(0x81) {
                Ok(e) => e.reader(4096),
                Err(_) => {
                    drop(iface);
                    continue;
                }
            };
            let out_ep = match iface.endpoint::<Bulk, Out>(0x01) {
                Ok(e) => e.writer(4096),
                Err(_) => {
                    drop(iface);
                    continue;
                }
            };

            let ctrl_if = iface.clone();
            let transport = MuxTransport::usb(in_ep, out_ep);
            return Ok(Self::from_transport(transport, ctrl_if));
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
        use tokio_stream::StreamExt as _;
        let mut rx = self.msg_rx.lock().await;
        let msg = rx.next().await.ok_or_else(|| anyhow!("channel closed"))?;
        debug!("MuxDevice::receive_msg got control message");
        Ok(msg)
    }

    pub async fn receive_snapshot(&self) -> Result<HVec<[u8; 6], MAX_DEVICES>> {
        use tokio_stream::StreamExt as _;
        let mut rx = self.snapshots_rx.lock().await;
        let devices = rx.next().await.ok_or_else(|| anyhow!("channel closed"))?;
        debug!("MuxDevice::receive_snapshot got device snapshot");
        Ok(devices)
    }

    pub async fn request_devices(&self) -> Result<HVec<[u8; 6], MAX_DEVICES>> {
        let mut last_err = None;
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }

            // Drain any stale snapshots that may be in the channel
            {
                use tokio_stream::StreamExt as _;
                let mut rx = self.snapshots_rx.lock().await;
                while let Ok(Some(_)) =
                    tokio::time::timeout(Duration::from_millis(10), rx.next()).await
                {
                    debug!("Drained stale snapshot");
                }
            }

            // Send the request
            if let Err(e) = self.send_msg(MuxMsg::RequestDevices()).await {
                last_err = Some(e);
                continue;
            }

            // Wait for response with a 1-second timeout
            let total_timeout = Duration::from_secs(1);

            match tokio::time::timeout(total_timeout, self.receive_snapshot()).await {
                Ok(Ok(devices)) => return Ok(devices),
                Ok(Err(e)) => last_err = Some(e),
                Err(_) => last_err = Some(anyhow!("timeout waiting for DevicesSnapshot")),
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
            .ctrl_recv_filtered(Duration::from_secs(2), |msg| {
                matches!(msg, UsbMuxCtrlMsg::ReadVersionResponse(_))
            })
            .await?
        {
            UsbMuxCtrlMsg::ReadVersionResponse(version) => Ok(version),
            other => Err(anyhow!("unexpected ctrl response: {other:?}")),
        }
    }
    /// Start pairing mode on the dongle with a timeout (ms).
    /// Start pairing mode on the dongle with a timeout (ms).
    pub async fn start_pairing(&self, timeout_ms: u32) -> Result<()> {
        use odyssey_protocol::control::usb_mux::StartPairing;
        self.ctrl_send(UsbMuxCtrlMsg::StartPairing(StartPairing { timeout_ms }))
            .await?;
        match self
            .ctrl_recv_filtered(Duration::from_secs(2), |msg| {
                matches!(msg, UsbMuxCtrlMsg::StartPairingResponse())
                    || matches!(msg, UsbMuxCtrlMsg::PairingResult(_))
            })
            .await?
        {
            UsbMuxCtrlMsg::StartPairingResponse() => Ok(()),
            UsbMuxCtrlMsg::PairingResult(
                odyssey_protocol::control::usb_mux::PairingOutcome::Ok(_addr),
            ) => {
                // Pairing finished immediately; treat as success so the caller can proceed
                Ok(())
            }
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
                .ctrl_recv_filtered(Duration::from_secs(120), |msg| {
                    matches!(msg, UsbMuxCtrlMsg::PairingResult(_))
                })
                .await?
            {
                UsbMuxCtrlMsg::PairingResult(
                    odyssey_protocol::control::usb_mux::PairingOutcome::Ok(device),
                ) => return Ok(PairingEvent::Result(device)),
                UsbMuxCtrlMsg::PairingResult(
                    odyssey_protocol::control::usb_mux::PairingOutcome::Err(PairingError::Timeout),
                ) => return Ok(PairingEvent::Timeout),
                UsbMuxCtrlMsg::PairingResult(
                    odyssey_protocol::control::usb_mux::PairingOutcome::Err(
                        PairingError::Cancelled,
                    ),
                ) => return Ok(PairingEvent::Cancelled),
                _ => { /* filtered */ }
            }
        }
    }

    pub async fn clear_bonds(&self) -> Result<()> {
        use std::time::{Duration, Instant};
        self.ctrl_send(UsbMuxCtrlMsg::ClearBonds()).await?;
        let start = Instant::now();
        let max_wait = Duration::from_secs(10);
        loop {
            match self
                .ctrl_recv_filtered(Duration::from_secs(2), |msg| {
                    matches!(msg, UsbMuxCtrlMsg::ClearBondsResponse(_))
                        || matches!(msg, UsbMuxCtrlMsg::PairingResult(_))
                })
                .await
            {
                Ok(msg) => match msg {
                    UsbMuxCtrlMsg::ClearBondsResponse(
                        odyssey_protocol::control::usb_mux::ClearBondsResult::Ok(),
                    ) => return Ok(()),
                    UsbMuxCtrlMsg::ClearBondsResponse(
                        odyssey_protocol::control::usb_mux::ClearBondsResult::Err(
                            ClearBondsError::Failed,
                        ),
                    ) => return Err(anyhow!("Failed to clear bonds")),
                    // Ignore unrelated ctrl messages (e.g., pairing result racing with clear)
                    _ if start.elapsed() < max_wait => continue,
                    // No explicit response but we've waited long enough; assume success because the
                    // dongle side clears synchronously and may drop the response when busy.
                    _ => {
                        eprintln!("Warning: no ClearBonds response received; assuming success");
                        return Ok(());
                    }
                },
                Err(e) => {
                    // Treat timeouts as benign while waiting; after max_wait, assume success.
                    if start.elapsed() < max_wait {
                        continue;
                    }
                    eprintln!(
                        "Warning: ClearBonds timeout waiting for ctrl event: {e}; assuming success"
                    );
                    return Ok(());
                }
            }
        }
    }

    pub async fn list_bonds(
        &self,
    ) -> Result<HVec<odyssey_protocol::control::BondedDevice, MAX_DEVICES>> {
        self.ctrl_send(UsbMuxCtrlMsg::ListBonds()).await?;
        match self
            .ctrl_recv_filtered(Duration::from_secs(2), |msg| {
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

    pub async fn add_bond(&self, entry: odyssey_protocol::control::BondEntry) -> Result<()> {
        self.ctrl_send(UsbMuxCtrlMsg::AddBond(entry)).await?;
        match self
            .ctrl_recv_filtered(Duration::from_secs(5), |msg| {
                matches!(msg, UsbMuxCtrlMsg::AddBondResponse(_))
            })
            .await?
        {
            UsbMuxCtrlMsg::AddBondResponse(odyssey_protocol::control::AddBondResult::Ok()) => {
                Ok(())
            }
            UsbMuxCtrlMsg::AddBondResponse(odyssey_protocol::control::AddBondResult::Err(e)) => {
                Err(anyhow!("AddBond failed: {e:?}"))
            }
            other => Err(anyhow!("unexpected ctrl response: {other:?}")),
        }
    }
}

#[derive(Debug, Clone)]
pub enum PairingEvent {
    Result(odyssey_protocol::control::BondedDevice),
    Timeout,
    Cancelled,
}
