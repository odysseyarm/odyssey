//! usb_link.rs — adapter for direct USB connection to device using VmDevice
//!
//! This implementation uses VmDevice's streaming API to multiplex different
//! packet types into a unified DeviceLink interface.

use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use futures::{stream::SelectAll, StreamExt};
use odyssey_hub_common::device::{Device, Transport};
use protodongers::{Packet, PacketData, PacketType, PropKind, Props, VendorData};
use tokio::sync::mpsc;

use crate::device_link::DeviceLink;

/// Wraps a VmDevice for direct USB connection
pub struct UsbLink {
    dev_meta: Device,
    usb: ats_usb::device::VmDevice,
    rx: mpsc::Receiver<Packet>,
}

impl UsbLink {
    pub async fn connect_usb(info: nusb::DeviceInfo) -> Result<Self> {
        let usb = ats_usb::device::VmDevice::connect_usb(info).await?;
        Self::from_connected_usb(usb).await
    }

    /// Create UsbLink from an already connected VmDevice
    pub async fn from_connected_usb(usb: ats_usb::device::VmDevice) -> Result<Self> {
        // Read UUID from device properties
        let props = usb.read_prop(PropKind::Uuid).await?;
        let uuid_bytes = match props {
            Props::Uuid(bytes) => bytes,
            _ => return Err(anyhow!("Expected Uuid prop, got {:?}", props)),
        };

        // Convert 6-byte UUID to u64
        let mut padded = [0u8; 8];
        padded[..6].copy_from_slice(&uuid_bytes);
        let uuid = u64::from_le_bytes(padded);

        let dev_meta = Device {
            uuid,
            transport: Transport::USB,
        };

        // Create channel for multiplexed packet stream
        let (tx, rx) = mpsc::channel::<Packet>(128);

        // Spawn task to multiplex all stream types into unified packet stream
        let usb_clone = usb.clone();
        tokio::spawn(async move {
            let mut fused = SelectAll::new();

            // Start all available streams and map them to Packet
            match usb_clone.stream_combined_markers().await {
                Ok(stream) => {
                    fused.push(
                        stream
                            .map(|report| Packet {
                                id: 0,
                                data: PacketData::CombinedMarkersReport(report),
                            })
                            .boxed(),
                    );
                }
                Err(e) => tracing::warn!("Failed to start combined markers stream: {}", e),
            }

            match usb_clone.stream_poc_markers().await {
                Ok(stream) => {
                    fused.push(
                        stream
                            .map(|report| Packet {
                                id: 0,
                                data: PacketData::PocMarkersReport(report),
                            })
                            .boxed(),
                    );
                }
                Err(e) => tracing::warn!("Failed to start PoC markers stream: {}", e),
            }

            match usb_clone.stream_accel().await {
                Ok(stream) => {
                    fused.push(
                        stream
                            .map(|report| Packet {
                                id: 0,
                                data: PacketData::AccelReport(report),
                            })
                            .boxed(),
                    );
                }
                Err(e) => tracing::warn!("Failed to start accel stream: {}", e),
            }

            match usb_clone.stream_impact().await {
                Ok(stream) => {
                    fused.push(
                        stream
                            .map(|report| Packet {
                                id: 0,
                                data: PacketData::ImpactReport(report),
                            })
                            .boxed(),
                    );
                }
                Err(e) => tracing::warn!("Failed to start impact stream: {}", e),
            }

            // Also attempt to stream vendor packet types in the vendor range.
            // VmDevice exposes per-packet-type streams; vendor types occupy the
            // tag range (exclusive) between PacketType::VendorStart() and PacketType::VendorEnd().
            // We'll attempt to open streams for the typical vendor tag range (0x81..=0xfe).
            // If a particular vendor-tag stream is unavailable, ignore it.
            for tag in 0x81u8..=0xfeu8 {
                // Try to create a stream for PacketType::Vendor(tag). Some devices may not expose
                // all vendor streams; ignore errors and continue.
                match usb_clone
                    .stream(protodongers::PacketType::Vendor(tag))
                    .await
                {
                    Ok(stream) => {
                        fused.push(
                            stream
                                .map(move |pd| match pd {
                                    PacketData::Vendor(t, v) => Packet {
                                        id: 0,
                                        data: PacketData::Vendor(t, v),
                                    },
                                    // In case the underlying stream yields something unexpected,
                                    // forward it as-is wrapped in a Packet.
                                    other => Packet { id: 0, data: other },
                                })
                                .boxed(),
                        );
                    }
                    Err(_e) => {
                        // ignore missing vendor stream for this tag
                    }
                }
            }

            // Forward all packets to the channel
            while let Some(pkt) = fused.next().await {
                if tx.send(pkt).await.is_err() {
                    break; // Receiver dropped
                }
            }
            tracing::info!("UsbLink stream multiplexer task ended");
        });

        Ok(Self { dev_meta, usb, rx })
    }
}

impl DeviceLink for UsbLink {
    fn device(&self) -> Device {
        self.dev_meta.clone()
    }

    fn recv<'a>(&'a mut self) -> BoxFuture<'a, anyhow::Result<Packet>> {
        Box::pin(async move {
            self.rx
                .recv()
                .await
                .ok_or_else(|| anyhow!("USB packet stream ended"))
        })
    }

    fn send<'a>(&'a mut self, pkt: Packet) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            // Handle common control packet types
            match pkt.data {
                PacketData::WriteRegister(wr) => {
                    self.usb
                        .write_register(wr.port, wr.bank, wr.address, wr.data)
                        .await?;
                }
                PacketData::ReadRegister(reg) => {
                    // Note: Response will come through the stream
                    let _ = self
                        .usb
                        .read_register(reg.port, reg.bank, reg.address)
                        .await?;
                }
                PacketData::WriteConfig(cfg) => {
                    // Convert to the crate's GeneralConfig type (not the wire module)
                    let wire: protodongers::GeneralConfig = cfg.clone().into();
                    self.usb.write_config(wire).await?;
                }
                PacketData::ReadConfig(kind) => {
                    // Note: Response will come through the stream
                    let _ = self.usb.read_config(kind).await?;
                }
                PacketData::WriteMode(mode) => {
                    self.usb.write_mode(mode).await?;
                }
                PacketData::FlashSettings() => {
                    self.usb.flash_settings().await?;
                }
                PacketData::StreamUpdate(_upd) => {
                    // Stream management is handled by VmDevice internally
                }
                PacketData::Vendor(tag, vendor) => {
                    self.usb
                        .write_vendor(tag, &vendor.data[..vendor.len as usize])
                        .await?;
                }
                _ => {
                    // Other packet types are either responses or not supported for send
                    return Err(anyhow!("Unsupported packet type for send"));
                }
            }
            Ok(())
        })
    }
}
