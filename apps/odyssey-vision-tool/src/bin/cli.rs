#![expect(
    clippy::print_stderr,
    reason = "command-line tool: stderr is its interface"
)]

use std::process::ExitCode;

use anyhow::{Context as _, Result};
use nusb::MaybeFuture as _;
use odyssey_protocol::control::device::TransportMode;
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::Port;

/// Nordic Semiconductor's USB vendor id.
const NORDIC_VENDOR_ID: u16 = 0x1915;
/// Product ids a direct device may enumerate with.
const VM_PRODUCT_IDS: [u16; 3] = [0x520F, 0x5210, 0x5211];

fn print_help() {
    eprintln!("Usage: ./cli <usb>");
    eprintln!();
    eprintln!("Connect to first available USB device and read product IDs");
}

/// Connects to the first device that is in USB transport mode.
/// Devices in other modes are reported and skipped.
async fn connect_first_usb_mode_device() -> Result<VmDevice> {
    let devices = nusb::list_devices()
        .wait()
        .context("failed to list USB devices")?;

    for info in devices.filter(|info| {
        info.vendor_id() == NORDIC_VENDOR_ID && VM_PRODUCT_IDS.contains(&info.product_id())
    }) {
        match VmDevice::probe_transport_mode(&info).await {
            Ok(TransportMode::Usb) => {
                return VmDevice::connect_usb(info)
                    .await
                    .context("failed to connect to device");
            }
            Ok(mode) => {
                eprintln!(
                    "Skipping {:04x}:{:04x} - transport mode {:?}",
                    info.vendor_id(),
                    info.product_id(),
                    mode
                );
            }
            Err(e) => {
                eprintln!(
                    "Skipping {:04x}:{:04x} - failed to probe mode: {e}",
                    info.vendor_id(),
                    info.product_id()
                );
            }
        }
    }

    anyhow::bail!("No device in USB mode found")
}

async fn run(device: &VmDevice) -> Result<()> {
    let nf_pid = device
        .product_id(Port::Nf)
        .await
        .context("failed to read nearfield product id")?;
    let wf_pid = device
        .product_id(Port::Wf)
        .await
        .context("failed to read widefield product id")?;
    eprintln!("Nearfield Product ID: 0x{nf_pid:x}");
    eprintln!("Widefield Product ID: 0x{wf_pid:x}");
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).map(String::as_str) != Some("usb") {
        print_help();
        return ExitCode::FAILURE;
    }

    let result = match connect_first_usb_mode_device().await {
        Ok(device) => run(&device).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}
