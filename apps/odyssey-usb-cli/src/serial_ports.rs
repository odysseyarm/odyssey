//! Discovery of the serial (CDC ACM) ports exposed by connected devices.

use serialport::SerialPortType;

use crate::usb_ids::is_vm_device;

/// USB interface number of the CDC ACM data module. The other interfaces are
/// its functional subordinate interface (1), the DFU module (2) and the DFU
/// subordinate interface (3).
const CDC_ACM_MODULE_INTERFACE: u8 = 0;

/// Names of the serial ports that belong to a connected device.
pub(crate) fn list_device_serial_ports() -> Vec<String> {
    let ports = match serialport::available_ports() {
        Ok(ports) => ports,
        Err(e) => {
            eprintln!("Failed to list serial ports: {e}");
            return vec![];
        }
    };

    let mut names = Vec::new();
    for port in ports {
        let SerialPortType::UsbPort(port_info) = &port.port_type else {
            continue;
        };
        if !is_vm_device(port_info.vid, port_info.pid) {
            continue;
        }
        let is_module_port = port_info
            .interface
            .is_none_or(|interface| interface == CDC_ACM_MODULE_INTERFACE);
        if is_module_port {
            names.push(port.port_name);
        }
    }
    names
}
