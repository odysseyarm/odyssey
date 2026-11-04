use anyhow::Result;
use nusb::DeviceInfo;

use super::mux_device::MuxDevice;
use super::vm_device::VmDevice;

/// Represents either a direct USB connection or a mux-mediated connection to a vision module
#[derive(Clone, Debug)]
pub enum VmConnectionInfo {
    DirectUsb(DeviceInfo),
    ViaMux {
        mux: MuxDevice,
        device_addr: [u8; 6],
    },
}

impl VmConnectionInfo {
    /// Connect to the vision module using the appropriate method
    pub async fn connect(self) -> Result<VmDevice> {
        match self {
            Self::DirectUsb(info) => VmDevice::connect_usb(info).await,
            Self::ViaMux { mux, device_addr } => VmDevice::connect_via_mux(mux, device_addr).await,
        }
    }

    /// Get a display string for this device
    pub fn display_string(&self) -> String {
        match self {
            Self::DirectUsb(info) => {
                format!(
                    "Vision Module {:04X}:{:04X}",
                    info.vendor_id(),
                    info.product_id()
                )
            }
            Self::ViaMux { device_addr, .. } => {
                format!(
                    "VM via Mux {:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
                    device_addr[0],
                    device_addr[1],
                    device_addr[2],
                    device_addr[3],
                    device_addr[4],
                    device_addr[5]
                )
            }
        }
    }
}
