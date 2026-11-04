//! USB enumeration shared by the `mux`, `device` and `bond` commands.

use nusb::{DeviceInfo, MaybeFuture as _};

/// Nordic Semiconductor's USB vendor id, used by every ATS device.
pub(crate) const NORDIC_VENDOR_ID: u16 = 0x1915;
/// USB product id of the mux (dongle).
const MUX_PRODUCT_ID: u16 = 0x5212;
/// USB product ids a direct (lite/vm) device may enumerate with.
const VM_PRODUCT_IDS: [u16; 3] = [0x520F, 0x5210, 0x5211];

fn list_usb_devices(keep: impl Fn(&DeviceInfo) -> bool) -> Result<Vec<DeviceInfo>, String> {
    Ok(nusb::list_devices()
        .wait()
        .map_err(|e| format!("Failed to list USB devices: {e}"))?
        .filter(keep)
        .collect())
}

pub(crate) fn list_muxes() -> Result<Vec<DeviceInfo>, String> {
    list_usb_devices(|info| {
        info.vendor_id() == NORDIC_VENDOR_ID && info.product_id() == MUX_PRODUCT_ID
    })
}

/// Every USB device that looks like a direct (lite/vm) device, whatever mode it is in.
pub(crate) fn list_vm_usb_devices() -> Result<Vec<DeviceInfo>, String> {
    list_usb_devices(|info| {
        info.vendor_id() == NORDIC_VENDOR_ID && VM_PRODUCT_IDS.contains(&info.product_id())
    })
}

pub(crate) fn list_nordic_devices() -> Result<Vec<DeviceInfo>, String> {
    list_usb_devices(|info| info.vendor_id() == NORDIC_VENDOR_ID)
}

pub(crate) fn product_name(info: &DeviceInfo) -> &str {
    info.product_string().unwrap_or("Unknown")
}

pub(crate) fn format_bd_addr(addr: &[u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        addr[0], addr[1], addr[2], addr[3], addr[4], addr[5]
    )
}
