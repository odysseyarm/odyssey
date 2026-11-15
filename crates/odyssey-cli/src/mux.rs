//! Commands for the mux dongle.

use std::time::Duration;

use clap::Subcommand;
use eros::Context as _;
use nusb::DeviceInfo;
use odyssey_usb::device::MuxDevice;

use crate::usb_discovery::{
    NORDIC_VENDOR_ID, format_bd_addr, list_muxes, list_nordic_devices, product_name,
};

/// USB product id of the vendors' Nordic DFU bootloaders.
const BOOTLOADER_PRODUCT_ID: u16 = 0x521F;
const BOOTLOADER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Subcommand)]
pub(crate) enum MuxCommands {
    /// List all available mux dongles
    ListDevices,
    /// List currently connected BLE devices
    List,
    /// Read firmware version
    Version,
    /// Print USB interfaces and endpoints for the mux
    UsbInfo,
    /// Start pairing mode with optional timeout (ms)
    Pair {
        #[arg(short, long, default_value_t = 120000)]
        timeout: u32,
    },
    /// Cancel pairing mode
    PairCancel,
    /// Clear all bonds
    ClearBonds,
    /// Reset into the vendor bootloader for a serial DFU update
    EnterBootloader,
}

fn select_mux(device_index: Option<usize>) -> eros::Result<DeviceInfo> {
    let devices = list_muxes()?;

    if devices.is_empty() {
        eros::bail!("No mux found (VID:0x1915 PID:0x5212). Please connect a mux.");
    }

    if let Some(idx) = device_index {
        return Ok(devices
            .get(idx)
            .ok_or_else(|| {
                eros::error!(
                    "Device index {idx} not found. Use 'mux list-devices' to see available devices."
                )
            })?
            .clone());
    }
    if devices.len() > 1 {
        eros::bail!(
            "Multiple muxes found ({}). Please specify which one to use with --device <index>. Use 'mux list-devices' to list them.",
            devices.len()
        );
    }
    Ok(devices[0].clone())
}

async fn connect_to_mux(device_index: Option<usize>) -> eros::Result<MuxDevice> {
    Ok(MuxDevice::connect_usb(select_mux(device_index)?)
        .await
        .context("Failed to connect to mux")?)
}

async fn cmd_enter_bootloader(info: DeviceInfo) -> eros::Result<()> {
    let device = MuxDevice::connect_usb(info.clone())
        .await
        .context("Failed to connect to mux")?;
    device.enter_bootloader().await.context(
        "The dongle refused the command; firmware before 0.1.15 needs its reset/DFU button",
    )?;
    drop(device);
    let start = std::time::Instant::now();
    while start.elapsed() < BOOTLOADER_TIMEOUT {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if list_nordic_devices()?.iter().any(|found| {
            found.vendor_id() == NORDIC_VENDOR_ID
                && found.product_id() == BOOTLOADER_PRODUCT_ID
                && found.bus_id() == info.bus_id()
                && found.port_chain() == info.port_chain()
        }) {
            println!("Dongle is in its bootloader");
            return Ok(());
        }
    }
    eros::bail!(
        "The dongle did not come back as its bootloader within {BOOTLOADER_TIMEOUT:?}; its bootloader may ignore the request, so press its reset/DFU button"
    )
}

fn cmd_list_devices() -> eros::Result<()> {
    let devices = list_muxes()?;

    if devices.is_empty() {
        println!("No muxes found (VID:0x1915 PID:0x5212)");
    } else {
        println!("Available muxes:");
        for (i, info) in devices.iter().enumerate() {
            println!(
                "  [{}] VID:0x{:04X} PID:0x{:04X} - {}",
                i,
                info.vendor_id(),
                info.product_id(),
                product_name(info)
            );
        }
        println!();
        println!("Use --device <index> to select a specific mux");
    }
    Ok(())
}

async fn cmd_list(device: &MuxDevice) -> eros::Result<()> {
    let devices = device
        .request_devices()
        .await
        .context("Failed to get devices")?;

    if devices.is_empty() {
        println!("No devices currently connected");
    } else {
        println!("Connected devices ({}):", devices.len());
        for dev in &devices {
            println!("  {}", format_bd_addr(*dev));
        }
    }
    Ok(())
}

async fn cmd_version(device: &MuxDevice) -> eros::Result<()> {
    let version = device
        .read_version()
        .await
        .context("Failed to read version")?;

    println!(
        "Firmware version: {}.{}.{}",
        version.firmware_semver[0], version.firmware_semver[1], version.firmware_semver[2]
    );
    println!(
        "Protocol version: {}.{}.{}",
        version.protocol_semver[0], version.protocol_semver[1], version.protocol_semver[2]
    );
    Ok(())
}

async fn cmd_pair(device: &MuxDevice, timeout: u32) -> eros::Result<()> {
    println!("Starting pairing for {timeout} ms...");
    device
        .start_pairing(timeout)
        .await
        .context("Failed to start pairing")?;
    println!("Pairing started. Waiting for result...");
    match device
        .wait_pairing_event()
        .await
        .context("Error waiting for pairing event")?
    {
        odyssey_usb::device::PairingEvent::Result(bond) => {
            println!("Paired with device {}", format_bd_addr(bond.uuid));
            Ok(())
        }
        odyssey_usb::device::PairingEvent::Timeout => {
            println!("Pairing timed out");
            Ok(())
        }
        odyssey_usb::device::PairingEvent::Cancelled => {
            println!("Pairing cancelled");
            Ok(())
        }
    }
}

async fn cmd_pair_cancel(device: &MuxDevice) -> eros::Result<()> {
    println!("Cancelling pairing mode...");
    device
        .cancel_pairing()
        .await
        .context("Failed to cancel pairing")?;
    println!("Pairing cancelled");
    Ok(())
}

async fn cmd_clear_bonds(device: &MuxDevice) -> eros::Result<()> {
    println!("Clearing bonds...");
    device
        .clear_bonds()
        .await
        .context("Failed to clear bonds")?;
    println!("Bonds cleared");
    Ok(())
}

async fn cmd_usb_info(device_index: Option<usize>) -> eros::Result<()> {
    use nusb::transfer::{Bulk, In, Interrupt, Out};
    let device_info = select_mux(device_index)?;
    let dev = device_info.open().await.context("Failed to open device")?;
    println!(
        "Probing interfaces for VID:0x{:04X} PID:0x{:04X}",
        device_info.vendor_id(),
        device_info.product_id()
    );
    for idx in 0u8..8 {
        if let Ok(iface) = dev.claim_interface(idx).await {
            let bulk_in = iface.endpoint::<Bulk, In>(0x81).is_ok();
            let bulk_out = iface.endpoint::<Bulk, Out>(0x01).is_ok();
            let intr_in = iface.endpoint::<Interrupt, In>(0x86).is_ok();
            println!(
                "  iface {idx}: bulk_out 0x01={bulk_out}, bulk_in 0x81={bulk_in}, intr_in 0x86={intr_in}"
            );
        }
    }
    Ok(())
}

pub(crate) async fn handle_command(
    device_index: Option<usize>,
    command: MuxCommands,
) -> eros::Result<()> {
    match command {
        MuxCommands::ListDevices => cmd_list_devices(),
        MuxCommands::UsbInfo => cmd_usb_info(device_index).await,
        MuxCommands::List => {
            let device = connect_to_mux(device_index).await?;
            cmd_list(&device).await
        }
        MuxCommands::Version => {
            let device = connect_to_mux(device_index).await?;
            cmd_version(&device).await
        }
        MuxCommands::Pair { timeout } => {
            let device = connect_to_mux(device_index).await?;
            cmd_pair(&device, timeout).await
        }
        MuxCommands::PairCancel => {
            let device = connect_to_mux(device_index).await?;
            cmd_pair_cancel(&device).await
        }
        MuxCommands::ClearBonds => {
            let device = connect_to_mux(device_index).await?;
            cmd_clear_bonds(&device).await
        }
        MuxCommands::EnterBootloader => cmd_enter_bootloader(select_mux(device_index)?).await,
    }
}
