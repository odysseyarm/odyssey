use nusb::DeviceInfo;

use super::hex_address::HexAddress;
use super::mux_device::MuxDevice;
use super::vm_device::VmDevice;
use crate::Result;

/// How to reach a vision module: directly over USB or through a dongle.
///
/// Since: Odyssey Desktop 0.6.0.
#[derive(Clone, Debug)]
pub enum VmConnectionInfo {
    /// Since: Odyssey Desktop 0.6.0.
    DirectUsb(DeviceInfo),
    /// Since: Odyssey Desktop 0.6.0.
    ViaMux {
        /// Since: Odyssey Desktop 0.6.0.
        mux: MuxDevice,
        /// Since: Odyssey Desktop 0.6.0.
        device_addr: [u8; 6],
    },
}

impl VmConnectionInfo {
    /// Connects using this USB or mux route.
    ///
    /// # Errors
    /// Returns the underlying USB, mux, or packet-transport error.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn connect(self) -> Result<VmDevice> {
        match self {
            Self::DirectUsb(info) => VmDevice::connect_usb(info).await,
            Self::ViaMux { mux, device_addr } => VmDevice::connect_via_mux(mux, device_addr).await,
        }
    }

    /// `Vision Module VID:PID` for a direct connection, the device address for
    /// one through a dongle.
    ///
    /// Since: Odyssey Desktop 0.6.0.
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
