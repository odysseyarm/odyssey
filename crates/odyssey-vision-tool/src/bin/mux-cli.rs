//! Mux CLI tool for managing the dongle

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "command-line tool: stdout/stderr is its interface"
)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use nusb::MaybeFuture as _;
use odyssey_usb::device::MuxDevice;

/// Nordic Semiconductor's USB vendor id.
const NORDIC_VENDOR_ID: u16 = 0x1915;
/// USB product id of the mux (dongle).
const MUX_PRODUCT_ID: u16 = 0x5212;

/// USB endpoints probed by `usb-info` (address of each expected endpoint).
const BULK_OUT_ENDPOINT: u8 = 0x01;
const BULK_IN_ENDPOINT: u8 = 0x81;
const INTERRUPT_IN_ENDPOINT: u8 = 0x86;
/// Interfaces `usb-info` tries to claim.
const PROBED_INTERFACE_COUNT: u8 = 8;

#[derive(Parser)]
#[command(name = "mux-cli")]
#[command(about = "CLI tool for managing the dongle", long_about = None)]
struct Cli {
    /// Index of the device to use (use 'devices' command to list)
    #[arg(short, long)]
    device: Option<usize>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print USB interfaces and endpoints for the dongle
    UsbInfo,
    /// List all available dongles
    Devices,
    /// List currently connected BLE devices
    List,
    /// Read firmware version
    Version,
    /// Start pairing mode with optional timeout (ms)
    Pair {
        #[arg(short, long, default_value_t = 120000)]
        timeout: u32,
    },
    /// Cancel pairing mode
    PairCancel,
    /// Clear bonds
    Clear,
}

fn list_dongles() -> Result<Vec<nusb::DeviceInfo>, String> {
    let devices: Vec<_> = nusb::list_devices()
        .wait()
        .map_err(|e| format!("Failed to list USB devices: {e}"))?
        .filter(|info| info.vendor_id() == NORDIC_VENDOR_ID && info.product_id() == MUX_PRODUCT_ID)
        .collect();

    Ok(devices)
}

/// The dongle selected by `--device`, or the only one connected.
fn select_dongle(
    devices: Vec<nusb::DeviceInfo>,
    device_index: Option<usize>,
) -> Result<nusb::DeviceInfo, String> {
    if let Some(idx) = device_index {
        return devices.get(idx).cloned().ok_or_else(|| {
            format!("Device index {idx} not found. Use 'devices' command to see available devices.")
        });
    }
    if devices.len() > 1 {
        return Err(format!(
            "Multiple dongles found ({}). Please specify which one to use with --device <index>. Use 'devices' command to list them.",
            devices.len()
        ));
    }
    devices.into_iter().next().ok_or_else(|| {
        "No dongle found (VID:0x1915 PID:0x5212). Please connect a dongle.".to_string()
    })
}

async fn connect_to_device(device_index: Option<usize>) -> Result<MuxDevice, String> {
    let devices = list_dongles()?;

    if devices.is_empty() {
        return Err(
            "No dongle found (VID:0x1915 PID:0x5212). Please connect a dongle.".to_string(),
        );
    }

    let device_info = select_dongle(devices, device_index)?;

    MuxDevice::connect_usb(device_info)
        .await
        .map_err(|e| format!("Failed to connect to dongle: {e}"))
}

fn format_device_addr(addr: &[u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        addr[0], addr[1], addr[2], addr[3], addr[4], addr[5]
    )
}

fn cmd_devices() -> Result<(), String> {
    let devices = list_dongles()?;

    if devices.is_empty() {
        println!("No dongles found (VID:0x1915 PID:0x5212)");
    } else {
        println!("Available dongles:");
        for (i, info) in devices.iter().enumerate() {
            let product = info
                .product_string()
                .map_or_else(|| "Unknown".to_string(), ToString::to_string);
            println!(
                "  [{}] VID:0x{:04X} PID:0x{:04X} - {}",
                i,
                info.vendor_id(),
                info.product_id(),
                product
            );
        }
        println!();
        println!("Use --device <index> to select a specific dongle");
    }
    Ok(())
}

async fn cmd_list(device: &MuxDevice) -> Result<(), String> {
    let devices = device
        .request_devices()
        .await
        .map_err(|e| format!("Failed to get devices: {e}"))?;

    if devices.is_empty() {
        println!("No devices currently connected");
    } else {
        println!("Connected devices ({}):", devices.len());
        for dev in &devices {
            println!("  {}", format_device_addr(dev));
        }
    }
    Ok(())
}

async fn cmd_version(device: &MuxDevice) -> Result<(), String> {
    let version = device
        .read_version()
        .await
        .map_err(|e| format!("Failed to read version: {e}"))?;

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

async fn cmd_pair(device: &MuxDevice, timeout: u32) -> Result<(), String> {
    println!("Starting pairing for {timeout} ms...");
    device
        .start_pairing(timeout)
        .await
        .map_err(|e| format!("Failed to start pairing: {e}"))?;
    println!("Pairing started. Waiting for result...");
    match device
        .wait_pairing_event()
        .await
        .map_err(|e| format!("Error waiting for pairing event: {e}"))?
    {
        odyssey_usb::device::PairingEvent::Result(bond) => {
            println!(
                "Paired with device {} ({})",
                format_device_addr(&bond.uuid),
                bond.name.as_str()
            );
        }
        odyssey_usb::device::PairingEvent::Timeout => return Err("Pairing timed out".to_string()),
        odyssey_usb::device::PairingEvent::Cancelled => return Err("Pairing cancelled".to_string()),
    }
    Ok(())
}

async fn cmd_pair_cancel(device: &MuxDevice) -> Result<(), String> {
    println!("Cancelling pairing mode...");
    device
        .cancel_pairing()
        .await
        .map_err(|e| format!("Failed to cancel pairing: {e}"))?;
    println!("Pairing cancelled");
    Ok(())
}

async fn cmd_clear(device: &MuxDevice) -> Result<(), String> {
    println!("Clearing bonds...");
    device
        .clear_bonds()
        .await
        .map_err(|e| format!("Failed to clear bonds: {e}"))?;
    println!("Bonds cleared");
    Ok(())
}

async fn cmd_usb_info(device_index: Option<usize>) -> Result<(), String> {
    use nusb::transfer::{Bulk, In, Interrupt, Out};
    let devices = list_dongles()?;
    if devices.is_empty() {
        return Err("No dongle found (VID:0x1915 PID:0x5212)".to_string());
    }
    let device_info = select_dongle(devices, device_index)?;
    let dev = device_info
        .open()
        .await
        .map_err(|e| format!("Failed to open device: {e}"))?;
    println!(
        "Probing interfaces for VID:0x{:04X} PID:0x{:04X}",
        device_info.vendor_id(),
        device_info.product_id()
    );
    for idx in 0..PROBED_INTERFACE_COUNT {
        if let Ok(iface) = dev.claim_interface(idx).await {
            let bulk_in = iface.endpoint::<Bulk, In>(BULK_IN_ENDPOINT).is_ok();
            let bulk_out = iface.endpoint::<Bulk, Out>(BULK_OUT_ENDPOINT).is_ok();
            let intr_in = iface
                .endpoint::<Interrupt, In>(INTERRUPT_IN_ENDPOINT)
                .is_ok();
            println!(
                "  iface {idx}: bulk_out 0x01={bulk_out}, bulk_in 0x81={bulk_in}, intr_in 0x86={intr_in}"
            );
        }
    }
    Ok(())
}

async fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Commands::Devices => cmd_devices(),
        Commands::UsbInfo => cmd_usb_info(cli.device).await,
        Commands::List => cmd_list(&connect_to_device(cli.device).await?).await,
        Commands::Version => cmd_version(&connect_to_device(cli.device).await?).await,
        Commands::Pair { timeout } => {
            cmd_pair(&connect_to_device(cli.device).await?, timeout).await
        }
        Commands::PairCancel => cmd_pair_cancel(&connect_to_device(cli.device).await?).await,
        Commands::Clear => cmd_clear(&connect_to_device(cli.device).await?).await,
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}
