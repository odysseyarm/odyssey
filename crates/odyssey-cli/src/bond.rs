//! `bond`: writes a matching bond entry to a dongle and a device by hand.

use clap::Args;
use eros::Context as _;
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

async fn list_vm_devices() -> eros::Result<Vec<nusb::DeviceInfo>> {
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

/// The first 6 bytes of the FICR device ID, parsed from the USB serial number. The
/// serial is 16 uppercase hex chars encoding 8 bytes, low word then high word.
fn bd_addr_from_serial(serial: &str) -> eros::Result<[u8; 6]> {
    if serial.len() < 12 {
        eros::bail!("Serial number too short to derive BD address: {serial}");
    }
    if !serial.is_ascii() {
        eros::bail!("USB serial number is not ASCII hexadecimal");
    }
    let mut addr = [0u8; 6];
    for i in 0..6 {
        addr[i] = u8::from_str_radix(&serial[i * 2..i * 2 + 2], 16)
            .with_context(|| format!("Failed to parse serial number byte {i}"))?;
    }
    Ok(addr)
}

async fn pick_mux(index: Option<usize>) -> eros::Result<(MuxDevice, [u8; 6])> {
    let muxes = list_muxes()?;
    if muxes.is_empty() {
        eros::bail!("No dongle found (VID:0x1915 PID:0x5212)");
    }
    let info = if let Some(i) = index {
        muxes
            .into_iter()
            .nth(i)
            .ok_or_else(|| eros::error!("Dongle index {i} not found"))?
    } else {
        if muxes.len() > 1 {
            eros::bail!(
                "Multiple dongles found ({}). Specify one with --mux <index>.",
                muxes.len()
            );
        }
        muxes
            .into_iter()
            .next()
            .ok_or_else(|| eros::error!("No dongle found"))?
    };
    let serial = info
        .serial_number()
        .ok_or_else(|| eros::error!("Dongle has no USB serial number"))?;
    let dongle_bd_addr = bd_addr_from_serial(serial)?;
    let mux = MuxDevice::connect_usb(info)
        .await
        .context("Failed to connect to dongle")?;
    Ok((mux, dongle_bd_addr))
}

async fn pick_device(index: Option<usize>) -> eros::Result<VmDevice> {
    // Keep explicit indices aligned with `device list-devices`, even when an
    // earlier device cannot be probed.
    let devs = if index.is_some() {
        list_vm_usb_devices()?
    } else {
        list_vm_devices().await?
    };
    if devs.is_empty() {
        eros::bail!("No device found.");
    }
    let info = if let Some(i) = index {
        devs.into_iter()
            .nth(i)
            .ok_or_else(|| eros::error!("Device index {i} not found"))?
    } else {
        if devs.len() > 1 {
            eros::bail!(
                "Multiple devices found ({}). Specify one with --device <index>.",
                devs.len()
            );
        }
        devs.into_iter()
            .next()
            .ok_or_else(|| eros::error!("No device found"))?
    };
    Ok(VmDevice::connect_usb_any_mode(info)
        .await
        .context("Failed to connect to device")?)
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

pub(crate) async fn handle_bond(args: BondArgs) -> eros::Result<()> {
    use rand::RngCore as _;

    let (mux, dongle_bd_addr) = pick_mux(args.mux).await?;
    let device = pick_device(args.device).await?;

    // The device UUID is also its BLE address.
    let device_bd_addr: [u8; 6] = device
        .read_uuid()
        .await
        .context("Failed to read device UUID")?;

    println!("Dongle BD addr: {}", format_bd_addr(dongle_bd_addr));
    println!("Device BD addr: {}", format_bd_addr(device_bd_addr));

    // Both sides of the bond hold the same LTK and IRK.
    let mut rng = rand::thread_rng();
    let mut ltk = [0u8; 16];
    let mut irk = [0u8; 16];
    rng.fill_bytes(&mut ltk);
    rng.fill_bytes(&mut irk);

    // The dongle's entry names the device it connects to.
    let dongle_entry = bond_entry(device_bd_addr, ltk, irk);

    // The device's entry names the dongle it expects to connect to it.
    let device_entry = bond_entry(dongle_bd_addr, ltk, irk);

    println!("Sending bond to dongle...");
    mux.add_bond(dongle_entry)
        .await
        .context("Failed to add bond to dongle")?;
    println!("Dongle: bond added");

    println!("Sending bond to device...");
    device
        .add_bond(device_entry)
        .await
        .context("Failed to add bond to device")?;
    println!("Device: bond added");

    println!(
        "Bond created successfully: dongle {} <-> device {}",
        format_bd_addr(dongle_bd_addr),
        format_bd_addr(device_bd_addr)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::bd_addr_from_serial;

    #[test]
    fn usb_serial_address_rejects_non_ascii_without_panicking() {
        bd_addr_from_serial("0é00000000000").unwrap_err();
        bd_addr_from_serial("not-hex-serial").unwrap_err();
        assert_eq!(
            bd_addr_from_serial("0123456789ABCDEF").ok(),
            Some([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB])
        );
    }
}
