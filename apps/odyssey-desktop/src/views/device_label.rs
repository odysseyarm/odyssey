//! How the app names a device or a Bluetooth address.

use odyssey_hub_common::device::{Device, DeviceCapabilities};

const PRODUCT_ID_ATS_VM: u16 = 0x520F;
const PRODUCT_ID_ATS_LITE: u16 = 0x5210;
const PRODUCT_ID_ATS_LITE1: u16 = 0x5211;

/// The product's name for its USB product id.
pub(crate) fn device_type(device: &Device) -> &'static str {
    match device.product_id {
        PRODUCT_ID_ATS_VM => "ATS VM",
        PRODUCT_ID_ATS_LITE => "ATS Lite",
        PRODUCT_ID_ATS_LITE1 => "ATS Lite1",
        _ => "Unknown",
    }
}

/// An address as colon-separated uppercase hex bytes.
pub(crate) fn format_address(address: &[u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        address[0], address[1], address[2], address[3], address[4], address[5]
    )
}

/// The device's product, name and address, with a DFU mark for a device that
/// can only be flashed.
pub(crate) fn device_label(device: &Device) -> String {
    let [a, b, c, d, e, f] = device.uuid;
    let addr = format!("{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}");
    let device_type = device_type(device);
    let is_dfu = device.capabilities.contains(DeviceCapabilities::DFU)
        && !device.capabilities.contains(DeviceCapabilities::CONTROL);
    let name = device.name();
    if is_dfu {
        format!("{device_type} [DFU] ({addr})")
    } else if name.is_empty() {
        format!("{device_type} ({addr})")
    } else {
        format!("{device_type}: {name} ({addr})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_uppercase_and_colon_separated() {
        assert_eq!(
            format_address(&[0, 1, 0xAB, 0xCD, 0xEF, 0xFF]),
            "00:01:AB:CD:EF:FF"
        );
    }
}
