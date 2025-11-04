//! usb_hub_link.rs — wraps ats_usb::device::HubDevice into DeviceLink
//!
//! The USB Hub/Dongle uses the HubMsg protocol to multiplex multiple devices
//! over a single USB connection. Each device connected to the hub gets packets
//! routed through HubMsg::SendTo and HubMsg::DevicePacket.

use std::future::Future;
use std::pin::Pin;

use anyhow::{anyhow, Result};
use ats_usb::device::HubDevice;
use ats_usb::packets::hub::HubMsg;
use nusb::DeviceInfo;
use odyssey_hub_common::device::{Device, Transport};
use protodongers::Packet;
use tokio::sync::mpsc;

use crate::device_link::DeviceLink;

/// Wraps a HubDevice connection to a specific device through the hub
pub struct UsbHubLink {
    dev_meta: Device,
    hub: HubDevice,
    device_addr: [u8; 6], // 6-byte BLE address
    rx: mpsc::Receiver<Packet>,
}

impl UsbHubLink {
    /// Connect to a device through the USB hub
    /// - `hub_info`: USB device info for the hub itself
    /// - `device_addr`: 6-byte BLE address of the target device
    pub async fn connect(hub_info: DeviceInfo, device_addr: [u8; 6]) -> Result<Self> {
        let hub = ats_usb::device::HubDevice::connect_usb(hub_info).await?;
        Self::from_connected_hub(hub, device_addr).await
    }

    /// Lightweight constructor: create a `UsbHubLink` from an already connected `HubDevice`
    /// using an externally-provided receiver. This allows a single hub manager task to own
    /// the HubDevice receive loop and route `HubMsg::DevicePacket` into per-device channels.
    pub fn from_connected_hub_with_rx(
        hub: ats_usb::device::HubDevice,
        device_addr: [u8; 6],
        rx: mpsc::Receiver<Packet>,
    ) -> Self {
        // Get device UUID from the 6-byte address
        let mut uuid_bytes = [0u8; 8];
        uuid_bytes[..6].copy_from_slice(&device_addr);
        let uuid = u64::from_le_bytes(uuid_bytes);

        let dev_meta = Device {
            uuid,
            transport: Transport::BLE, // Devices connected through hub are BLE
        };

        Self {
            dev_meta,
            hub,
            device_addr,
            rx,
        }
    }

    /// Convenience constructor: create a `UsbHubLink` from a connected `HubDevice` and
    /// create an internal receiver channel for it. Use this when you don't need to
    /// supply your own receiver/routing mechanism.
    pub fn from_connected_hub_no_spawn(
        hub: ats_usb::device::HubDevice,
        device_addr: [u8; 6],
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Packet>(128);
        // tx is intentionally unused here — caller won't receive packets unless they
        // arrange to route hub messages into the created channel. This constructor is
        // useful for quick local wiring where the link manages its own receive task
        // elsewhere (or for tests). If you need the hub manager to route packets,
        // prefer `from_connected_hub_with_rx`.
        let _ = tx;
        Self::from_connected_hub_with_rx(hub, device_addr, rx)
    }

    /// Create UsbHubLink from an already connected HubDevice
    pub async fn from_connected_hub(
        hub: ats_usb::device::HubDevice,
        device_addr: [u8; 6],
    ) -> Result<Self> {
        // Get device UUID from the 6-byte address
        let mut uuid_bytes = [0u8; 8];
        uuid_bytes[..6].copy_from_slice(&device_addr);
        let uuid = u64::from_le_bytes(uuid_bytes);

        let dev_meta = Device {
            uuid,
            transport: Transport::BLE, // Devices connected through hub are BLE
        };

        // Create packet stream from hub messages
        let (tx, rx) = mpsc::channel::<Packet>(128);

        // Spawn task to listen for HubMsg::RecvFrom and forward to channel
        let hub_clone = hub.clone();
        let device_addr_clone = device_addr;
        tokio::spawn(async move {
            loop {
                match hub_clone.receive_msg().await {
                    Ok(msg) => {
                        // Use the hub message definition from the ats_usb crate's packets module
                        match msg {
                            HubMsg::DevicePacket(dev_pkt) => {
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
                        tracing::warn!("HubDevice receive error: {}", e);
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
            hub,
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
                .ok_or_else(|| anyhow!("USB Hub packet stream ended"))
        })
    }

    fn send<'a>(
        &'a mut self,
        pkt: Packet,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // Send packet to specific device through the hub
            self.hub.send_to(self.device_addr, pkt).await?;
            Ok(())
        })
    }
}
