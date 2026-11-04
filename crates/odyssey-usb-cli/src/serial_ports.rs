//! Discovery of the serial (CDC ACM) ports exposed by connected devices.

use eros::Context as _;
use serialport::SerialPortType;

use crate::usb_ids::is_vm_device;

/// USB interface number of the CDC ACM data module. The other interfaces are
/// its functional subordinate interface (1), the DFU module (2) and the DFU
/// subordinate interface (3).
const CDC_ACM_MODULE_INTERFACE: u8 = 0;

/// Resolve an explicit CDC ACM path to its USB identity. Never guess which USB
/// device a path belongs to: calibration must not target a different device.
pub(crate) fn usb_identity(path: &str) -> eros::Result<serialport::UsbPortInfo> {
    let canonical_path = std::fs::canonicalize(path).ok();
    let port = serialport::available_ports()
        .context("Failed to list serial ports")?
        .into_iter()
        .find(|port| {
            port.port_name == path
                || canonical_path.as_ref().is_some_and(|requested| {
                    std::fs::canonicalize(&port.port_name)
                        .is_ok_and(|candidate| candidate == *requested)
                })
        })
        .ok_or_else(|| eros::error!("Serial port {path:?} not found"))?;
    let SerialPortType::UsbPort(info) = port.port_type else {
        eros::bail!("Serial port {path:?} is not a USB device");
    };
    if !is_vm_device(info.vid, info.pid)
        || info
            .interface
            .is_some_and(|interface| interface != CDC_ACM_MODULE_INTERFACE)
    {
        eros::bail!("Serial port {path:?} is not an Odyssey device port");
    }
    if info.serial_number.as_deref().is_none_or(str::is_empty) {
        eros::bail!(
            "Serial port {path:?} has no USB serial number; cannot identify its device safely"
        );
    }
    Ok(info)
}
