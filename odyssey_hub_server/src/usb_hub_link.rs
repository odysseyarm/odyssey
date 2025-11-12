//! usb_mux_link.rs — wraps ats_usb::device::MuxDevice into DeviceLink
//!
//! The USB Mux/Dongle uses the MuxMsg protocol to multiplex multiple devices
//! over a single USB connection. Each device connected to the mux gets packets
//! routed through MuxMsg::SendTo and MuxMsg::DevicePacket.

use std::future::Future;
use std::pin::Pin;

use anyhow::{anyhow, Result};
use ats_usb::device::MuxDevice;
use ats_usb::packets::mux::MuxMsg;
use ats_usb::packets::vm::Packet;
use nusb::DeviceInfo;
use odyssey_hub_common::device::{Device, Transport};
use tokio::sync::mpsc;

use crate::device_link::DeviceLink;

/// Wraps a MuxDevice connection to a specific device through the mux
pub struct UsbHubLink {
    dev_meta: Device,
    mux: MuxDevice,
    device_addr: [u8; 6], // 6-byte BLE address
    rx: mpsc::Receiver<Packet>,
}

impl UsbHubLink {
    /// Connect to a device through the USB Mux
    /// - `mux_info`: USB device info for the mux itself
    /// - `device_addr`: 6-byte BLE address of the target device
    pub async fn connect(mux_info: DeviceInfo, device_addr: [u8; 6]) -> Result<Self> {
        let mux = ats_usb::device::MuxDevice::connect_usb(mux_info).await?;
        Self::from_connected_mux(mux, device_addr).await
    }

    /// Lightweight constructor: create a `UsbHubLink` from an already connected `MuxDevice`
    /// using an externally-provided receiver. This allows a single mux manager task to own
    /// the MuxDevice receive loop and route `MuxMsg::DevicePacket` into per-device channels.
    ///
    /// NOTE: This uses a temporary UUID derived from the BLE address. Call `read_uuid()`
    /// after creation to get the real device UUID.
    pub fn from_connected_mux_with_rx(
        mux: ats_usb::device::MuxDevice,
        device_addr: [u8; 6],
        rx: mpsc::Receiver<Packet>,
    ) -> Self {
        // Use the BLE address as the UUID
        let dev_meta = Device {
            uuid: device_addr,
            transport: Transport::UsbHub,
        };

        Self {
            dev_meta,
            mux,
            device_addr,
            rx,
        }
    }

    /// Read the actual UUID from the device and update the internal device metadata
    pub async fn read_and_set_uuid(&mut self) -> Result<[u8; 6]> {
        use ats_usb::packets::vm::{Packet, PacketData, PropKind, Props};

        // Send ReadProp request for UUID
        let read_pkt = Packet {
            id: 0,
            data: PacketData::ReadProp(PropKind::Uuid),
        };
        self.mux.send_to(self.device_addr, read_pkt).await?;

        // Wait for response
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Some(pkt) = self.rx.recv().await {
                    if let PacketData::ReadPropResponse(Props::Uuid(uuid_bytes)) = pkt.data {
                        return Ok(uuid_bytes);
                    }
                } else {
                    return Err(anyhow::anyhow!("Channel closed while reading UUID"));
                }
            }
        })
        .await??;

        // Update device metadata
        self.dev_meta.uuid = timeout;

        Ok(timeout)
    }

    /// Convenience constructor: create a `UsbHubLink` from a connected `MuxDevice` and
    /// create an internal receiver channel for it. Use this when you don't need to
    /// supply your own receiver/routing mechanism.
    pub fn from_connected_mux_no_spawn(
        mux: ats_usb::device::MuxDevice,
        device_addr: [u8; 6],
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Packet>(128);
        // tx is intentionally unused here — caller won't receive packets unless they
        // arrange to route mux messages into the created channel. This constructor is
        // useful for quick local wiring where the link manages its own receive task
        // elsewhere (or for tests). If you need the mux manager to route packets,
        // prefer `from_connected_mux_with_rx`.
        let _ = tx;
        Self::from_connected_mux_with_rx(mux, device_addr, rx)
    }

    /// Create UsbHubLink from an already connected MuxDevice
    pub async fn from_connected_mux(
        mux: ats_usb::device::MuxDevice,
        device_addr: [u8; 6],
    ) -> Result<Self> {
        let dev_meta = Device {
            uuid: device_addr,
            transport: Transport::UsbHub,
        };

        // Create packet stream from mux messages
        let (tx, rx) = mpsc::channel::<Packet>(128);

        // Spawn task to listen for MuxMsg::RecvFrom and forward to channel
        let mux_clone = mux.clone();
        let device_addr_clone = device_addr;
        tokio::spawn(async move {
            loop {
                match mux_clone.receive_msg().await {
                    Ok(msg) => {
                        // Use the mux message definition from the ats_usb crate's packets module
                        match msg {
                            MuxMsg::DevicePacket(dev_pkt) => {
                                // Check if this message is for our device
                                if dev_pkt.dev == device_addr_clone {
                                    if tx.send(dev_pkt.pkt).await.is_err() {
                                        break; // Receiver dropped
                                    }
                                }
                                // Ignore messages for other devices
                            }
                            _ => {
                                // Ignore other message types (they're handled elsewhere)
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("MuxDevice receive error: {}", e);
                        break;
                    }
                }
            }
            tracing::info!(
                "UsbHubLink receive task ended for device {:?}",
                device_addr_clone
            );
        });

        Ok(Self {
            dev_meta,
            mux,
            device_addr,
            rx,
        })
    }
}

impl DeviceLink for UsbHubLink {
    fn device(&self) -> Device {
        self.dev_meta.clone()
    }

    fn recv<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = anyhow::Result<Packet>> + Send + 'a>> {
        Box::pin(async move {
            self.rx
                .recv()
                .await
                .ok_or_else(|| anyhow!("USB Mux packet stream ended"))
        })
    }

    fn send<'a>(
        &'a mut self,
        pkt: Packet,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // Send packet to specific device through the mux
            self.mux.send_to(self.device_addr, pkt).await?;
            Ok(())
        })
    }
}
