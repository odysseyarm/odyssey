use anyhow::Result;
use nusb::DeviceInfo;

use super::hex_address::HexAddress;
use super::mux_device::MuxDevice;
use super::vm_device::VmDevice;

/// How to reach a vision module: directly over USB or through a dongle.
#[derive(Clone, Debug)]
pub enum VmConnectionInfo {
    DirectUsb(DeviceInfo),
    ViaMux {
        mux: MuxDevice,
        device_addr: [u8; 6],
    },
}

impl VmConnectionInfo {
    pub async fn connect(self) -> Result<VmDevice> {
        match self {
            Self::DirectUsb(info) => VmDevice::connect_usb(info).await,
            Self::ViaMux { mux, device_addr } => VmDevice::connect_via_mux(mux, device_addr).await,
        }
    }

    /// `Vision Module VID:PID` for a direct connection, the device address for
    /// one through a dongle.
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
                format!("VM via Mux {:#}", HexAddress(*device_addr))
            }
        }
    }
}
