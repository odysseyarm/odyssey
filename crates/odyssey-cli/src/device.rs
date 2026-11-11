//! Commands for lite/vm devices connected directly over USB.

use clap::Subcommand;
use eros::Context as _;
use odyssey_protocol::control::device::TransportMode;
use odyssey_usb::device::VmDevice;

use crate::usb_discovery::{
    format_bd_addr, list_nordic_devices, list_vm_usb_devices, product_name,
};

#[derive(Subcommand)]
pub(crate) enum DeviceCommands {
    /// List all available devices
    ListDevices,
    /// Read firmware version
    Version,
    /// Start pairing mode with optional timeout (ms)
    Pair {
        #[arg(short, long, default_value_t = 120000)]
        timeout: u32,
    },
    /// Cancel pairing mode
    PairCancel,
    /// Clear bond (for lite devices with single bond)
    ClearBond,
    /// Calibrate accelerometer
    AccelCalib {
        /// Number of samples per orientation
        #[arg(short, default_value_t = 400)]
        samples: usize,
        /// Gravity value
        #[arg(short, default_value_t = 9.80665)]
        gravity: f64,
        /// Output JSON file
        output: String,
    },
    /// Calibrate gyroscope
    GyroCalib {
        /// Number of samples
        #[arg(short, default_value_t = 400)]
        samples: usize,
        /// Output JSON file
        output: String,
    },
    /// Stream accelerometer data
    Stream,
}

async fn list_devices(require_usb_mode: bool) -> eros::Result<Vec<nusb::DeviceInfo>> {
    let mut devices = Vec::new();
    let mut non_usb_devices = 0usize;
    for info in list_vm_usb_devices()? {
        match VmDevice::probe_transport_mode(&info).await {
            Ok(mode) => {
                if require_usb_mode && !matches!(mode, TransportMode::Usb) {
                    non_usb_devices += 1;
                    continue;
                }
                if !matches!(mode, TransportMode::Usb) {
                    eprintln!(
                        "Info: {:04X}:{:04X} is in {:?} mode",
                        info.vendor_id(),
                        info.product_id(),
                        mode
                    );
                }
                devices.push(info);
            }
            Err(e) => {
                eprintln!(
                    "Skipping device {:04X}:{:04X}: transport probe failed: {e}",
                    info.vendor_id(),
                    info.product_id()
                );
            }
        }
    }
    if require_usb_mode && devices.is_empty() && non_usb_devices > 0 {
        eprintln!(
            "Found {non_usb_devices} device(s) not in USB mode; no USB-mode devices available"
        );
    }
    Ok(devices)
}

async fn connect_to_device(
    device_index: Option<usize>,
    require_usb_mode: bool,
) -> eros::Result<VmDevice> {
    // Explicit indices refer to the unfiltered enumeration printed by
    // `list-devices`. Probing or filtering first would renumber devices.
    let device_info = if let Some(idx) = device_index {
        list_vm_usb_devices()?
            .get(idx)
            .ok_or_else(|| {
                eros::error!(
                    "Device index {idx} not found. Use 'device list-devices' to see available devices."
                )
            })?
            .clone()
    } else {
        let devices = list_devices(require_usb_mode).await?;
        if devices.is_empty() {
            eros::bail!("No device found. Please connect a device.");
        }
        if devices.len() > 1 {
            eros::bail!(
                "Multiple devices found ({}). Please specify which one to use with --device <index>. Use 'device list-devices' to list them.",
                devices.len()
            );
        }
        devices[0].clone()
    };

    if require_usb_mode {
        Ok(VmDevice::connect_usb(device_info)
            .await
            .context("Failed to connect to device")?)
    } else {
        Ok(VmDevice::connect_usb_any_mode(device_info)
            .await
            .context("Failed to connect to device")?)
    }
}

async fn cmd_list_devices() -> eros::Result<()> {
    let all_usb_devices = list_vm_usb_devices()?;

    if all_usb_devices.is_empty() {
        println!("No devices found (VID:0x1915, PID:0x520F/0x5210/0x5211)");

        // With no match, list every Nordic device as a debugging aid.
        let nordic_devices = list_nordic_devices()?;

        if !nordic_devices.is_empty() {
            println!("\nOther Nordic (VID:0x1915) devices found:");
            for info in nordic_devices {
                println!(
                    "  VID:0x{:04X} PID:0x{:04X} - {}",
                    info.vendor_id(),
                    info.product_id(),
                    product_name(&info)
                );
            }
        }
    } else {
        println!("Found {} USB device(s):", all_usb_devices.len());
        for (i, info) in all_usb_devices.iter().enumerate() {
            println!(
                "  [{}] VID:0x{:04X} PID:0x{:04X} - {}",
                i,
                info.vendor_id(),
                info.product_id(),
                product_name(info)
            );
            println!(
                "      Bus: {:?}, Address: {}",
                info.bus_id(),
                info.device_address()
            );

            match VmDevice::probe_transport_mode(info).await {
                Ok(mode) => println!("      Transport mode: {mode:?}"),
                Err(e) => println!("      Transport mode: probe failed ({e})"),
            }

            println!("      Interfaces:");
            for iface in info.interfaces() {
                println!(
                    "        Interface {}: class=0x{:02X} subclass=0x{:02X}",
                    iface.interface_number(),
                    iface.class(),
                    iface.subclass()
                );
            }
        }
        println!();
        println!("Use --device <index> to select a specific device");
    }
    Ok(())
}

async fn cmd_version(device: &VmDevice) -> eros::Result<()> {
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

async fn cmd_pair(device: &VmDevice, timeout: u32) -> eros::Result<()> {
    println!("Starting pairing mode (timeout: {timeout}ms)...");

    device
        .start_pairing(timeout)
        .await
        .context("Failed to start pairing")?;
    println!("Pairing mode started, waiting for device...");

    let bond = device
        .wait_pairing_event()
        .await
        .context("Pairing failed")?;
    println!(
        "Successfully paired with device: {}",
        format_bd_addr(bond.uuid)
    );
    Ok(())
}

async fn cmd_pair_cancel(device: &VmDevice) -> eros::Result<()> {
    device
        .cancel_pairing()
        .await
        .context("Failed to cancel pairing")?;
    println!("Pairing cancelled");
    Ok(())
}

async fn cmd_clear_bond(device: &VmDevice) -> eros::Result<()> {
    device.clear_bond().await.context("Failed to clear bond")?;
    println!("Bond cleared successfully");
    Ok(())
}
pub(crate) async fn handle_command(
    device_index: Option<usize>,
    command: DeviceCommands,
) -> eros::Result<()> {
    match command {
        DeviceCommands::ListDevices => cmd_list_devices().await,
        DeviceCommands::Version => {
            let device = connect_to_device(device_index, true).await?;
            cmd_version(&device).await
        }
        DeviceCommands::Pair { timeout } => {
            let device = connect_to_device(device_index, false).await?;
            cmd_pair(&device, timeout).await
        }
        DeviceCommands::PairCancel => {
            let device = connect_to_device(device_index, false).await?;
            cmd_pair_cancel(&device).await
        }
        DeviceCommands::ClearBond => {
            let device = connect_to_device(device_index, false).await?;
            cmd_clear_bond(&device).await
        }
        DeviceCommands::AccelCalib {
            samples,
            gravity,
            output,
        } => {
            crate::calibration::validate_sample_count(samples)?;
            crate::calibration::validate_gravity(gravity)?;
            let mut device = connect_to_device(device_index, true).await?;
            crate::calibration::cmd_accel_calib(&mut device, samples, gravity, &output).await
        }
        DeviceCommands::GyroCalib { samples, output } => {
            crate::calibration::validate_sample_count(samples)?;
            let mut device = connect_to_device(device_index, true).await?;
            crate::calibration::cmd_gyro_calib(&mut device, samples, &output).await
        }
        DeviceCommands::Stream => {
            let mut device = connect_to_device(device_index, true).await?;
            crate::calibration::cmd_stream(&mut device).await
        }
    }
}
