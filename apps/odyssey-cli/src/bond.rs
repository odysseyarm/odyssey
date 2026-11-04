//! Bond command — manually create a bond between a dongle and a device

use clap::Args;
use odyssey_usb::device::{MuxDevice, VmDevice};

use crate::usb_discovery::{format_bd_addr, list_muxes, list_vm_usb_devices};

/// BLE security level 2: encrypted and authenticated.
const SECURITY_LEVEL_ENCRYPTED_AUTHENTICATED: u8 = 2;

#[derive(Args)]
pub(crate) struct BondArgs {
    /// Index of the dongle to use (use 'mux list-devices' to see available dongles)
    #[arg(short = 'm', long)]
    pub mux: Option<usize>,

    /// Index of the device to use (use 'device list-devices' to see available devices)
    #[arg(short = 'd', long)]
    pub device: Option<usize>,
}

async fn list_vm_devices() -> Result<Vec<nusb::DeviceInfo>, String> {
    let mut devices = Vec::new();
    for info in list_vm_usb_devices()? {
        match VmDevice::probe_transport_mode(&info).await {
            Ok(_) => devices.push(info),
            Err(e) => eprintln!(
                "Skipping {:04X}:{:04X}: probe failed: {e}",
                info.vendor_id(),
                info.product_id()
            ),
        }
    }
    Ok(devices)
}

/// Parse the first 6 bytes of the FICR device ID from the USB serial number string.
/// The serial is 16 uppercase hex chars representing 8 bytes (low word then high word).
fn bd_addr_from_serial(serial: &str) -> Result<[u8; 6], String> {
    if serial.len() < 12 {
        return Err(format!(
            "Serial number too short to derive BD address: {serial}"
        ));
    }
    if !serial.is_ascii() {
        return Err("USB serial number is not ASCII hexadecimal".to_string());
    }
    let mut addr = [0u8; 6];
    for i in 0..6 {
        addr[i] = u8::from_str_radix(&serial[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("Failed to parse serial number byte {i}: {e}"))?;
    }
    Ok(addr)
}

async fn pick_mux(index: Option<usize>) -> Result<(MuxDevice, [u8; 6]), String> {
    let muxes = list_muxes()?;
    if muxes.is_empty() {
        return Err("No dongle found (VID:0x1915 PID:0x5212)".to_string());
    }
    let info = match index {
        Some(i) => muxes
            .into_iter()
            .nth(i)
            .ok_or_else(|| format!("Dongle index {i} not found"))?,
        None => {
            if muxes.len() > 1 {
                return Err(format!(
                    "Multiple dongles found ({}). Specify one with --mux <index>.",
                    muxes.len()
                ));
            }
            muxes
                .into_iter()
                .next()
                .ok_or_else(|| "No dongle found".to_string())?
        }
    };
    let serial = info
        .serial_number()
        .ok_or_else(|| "Dongle has no USB serial number".to_string())?;
    let dongle_bd_addr = bd_addr_from_serial(serial)?;
    let mux = MuxDevice::connect_usb(info)
        .await
        .map_err(|e| format!("Failed to connect to dongle: {e}"))?;
    Ok((mux, dongle_bd_addr))
}

async fn pick_device(index: Option<usize>) -> Result<VmDevice, String> {
    // Keep explicit indices aligned with `device list-devices`, even when an
    // earlier device cannot be probed.
    let devs = if index.is_some() {
        list_vm_usb_devices()?
    } else {
        list_vm_devices().await?
    };
    if devs.is_empty() {
        return Err("No device found.".to_string());
    }
    let info = match index {
        Some(i) => devs
            .into_iter()
            .nth(i)
            .ok_or_else(|| format!("Device index {i} not found"))?,
        None => {
            if devs.len() > 1 {
                return Err(format!(
                    "Multiple devices found ({}). Specify one with --device <index>.",
                    devs.len()
                ));
            }
            devs.into_iter()
                .next()
                .ok_or_else(|| "No device found".to_string())?
        }
    };
    VmDevice::connect_usb_any_mode(info)
        .await
        .map_err(|e| format!("Failed to connect to device: {e}"))
}

fn bond_entry(
    bd_addr: [u8; 6],
    ltk: [u8; 16],
    irk: [u8; 16],
) -> odyssey_protocol::control::BondEntry {
    odyssey_protocol::control::BondEntry {
        bd_addr,
        ltk,
        security_level: SECURITY_LEVEL_ENCRYPTED_AUTHENTICATED,
        is_bonded: true,
        irk: Some(irk),
    }
}

pub(crate) async fn handle_bond(args: BondArgs) -> Result<(), String> {
    use rand::RngCore as _;

    let (mux, dongle_bd_addr) = pick_mux(args.mux).await?;
    let device = pick_device(args.device).await?;

    // Read device UUID — this is also its BLE address
    let device_bd_addr: [u8; 6] = device
        .read_uuid()
        .await
        .map_err(|e| format!("Failed to read device UUID: {e}"))?;

    println!("Dongle BD addr: {}", format_bd_addr(&dongle_bd_addr));
    println!("Device BD addr: {}", format_bd_addr(&device_bd_addr));

    // Generate random LTK and IRK (same values for both sides)
    let mut rng = rand::thread_rng();
    let mut ltk = [0u8; 16];
    let mut irk = [0u8; 16];
    rng.fill_bytes(&mut ltk);
    rng.fill_bytes(&mut irk);

    // Dongle bond: bd_addr = device (who the dongle connects to)
    let dongle_entry = bond_entry(device_bd_addr, ltk, irk);

    // Device bond: bd_addr = dongle (who the device expects to connect to it)
    let device_entry = bond_entry(dongle_bd_addr, ltk, irk);

    println!("Sending bond to dongle...");
    mux.add_bond(dongle_entry)
        .await
        .map_err(|e| format!("Failed to add bond to dongle: {e}"))?;
    println!("Dongle: bond added");

    println!("Sending bond to device...");
    device
        .add_bond(device_entry)
        .await
        .map_err(|e| format!("Failed to add bond to device: {e}"))?;
    println!("Device: bond added");

    println!(
        "Bond created successfully: dongle {} <-> device {}",
        format_bd_addr(&dongle_bd_addr),
        format_bd_addr(&device_bd_addr)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::bd_addr_from_serial;

    #[test]
    fn usb_serial_address_rejects_non_ascii_without_panicking() {
        assert!(bd_addr_from_serial("0é00000000000").is_err());
        assert!(bd_addr_from_serial("not-hex-serial").is_err());
        assert_eq!(
            bd_addr_from_serial("0123456789ABCDEF").ok(),
            Some([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB])
        );
    }
}
