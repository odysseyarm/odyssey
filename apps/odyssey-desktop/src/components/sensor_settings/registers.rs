use dioxus::prelude::ReadableExt as _;
use odyssey_hub_common::device::Device;

use crate::hub::HubContext;

pub(super) async fn read_u16_reg(
    hub: &HubContext,
    device: &Device,
    port: u8,
    bank: u8,
    addrs: [u8; 2],
) -> anyhow::Result<u16> {
    let lo = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, bank, addrs[0])
        .await?;
    let hi = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, bank, addrs[1])
        .await?;
    Ok(u16::from_le_bytes([lo, hi]))
}

pub(super) async fn write_u16_reg(
    hub: &HubContext,
    device: &Device,
    port: u8,
    bank: u8,
    addrs: [u8; 2],
    val: u16,
) -> anyhow::Result<()> {
    let bytes = val.to_le_bytes();
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, bank, addrs[0], bytes[0])
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, bank, addrs[1], bytes[1])
        .await?;
    Ok(())
}

pub(super) async fn read_u32_3byte(
    hub: &HubContext,
    device: &Device,
    port: u8,
    bank: u8,
    addrs: [u8; 3],
) -> anyhow::Result<u32> {
    let b0 = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, bank, addrs[0])
        .await?;
    let b1 = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, bank, addrs[1])
        .await?;
    let b2 = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, bank, addrs[2])
        .await?;
    Ok(u32::from_le_bytes([b0, b1, b2, 0]))
}

pub(super) async fn write_u32_3byte(
    hub: &HubContext,
    device: &Device,
    port: u8,
    bank: u8,
    addrs: [u8; 3],
    val: u32,
) -> anyhow::Result<()> {
    let bytes = val.to_le_bytes();
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, bank, addrs[0], bytes[0])
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, bank, addrs[1], bytes[1])
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, bank, addrs[2], bytes[2])
        .await?;
    Ok(())
}
