use heapless::Vec;

use crate::Packet;

pub const MAX_DEVICES: usize = 3;
pub type Uuid = [u8; 6];

const SEMVER: [u16; 3] = [0, 1, 0];

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub enum MuxMsg {
    DevicesSnapshot(Vec<Uuid, MAX_DEVICES>),
    DevicePacket(DevicePacket),
    RequestDevices,
    SendTo(SendTo),
    ReadVersion(),
    ReadVersionResponse(Version),
    /// Subscribe to device list changes. After subscribing, the dongle will send
    /// DevicesSnapshot messages whenever devices connect or disconnect.
    SubscribeDeviceList,
    /// Unsubscribe from device list changes.
    UnsubscribeDeviceList,
}

#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct DevicePacket {
    pub dev: Uuid,
    pub pkt: Packet,
}

#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct SendTo {
    pub dev: Uuid,
    pub pkt: Packet,
}

#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct Version {
    pub protocol_semver: [u16; 3],
    pub firmware_semver: [u16; 3],
}

impl Version {
    pub fn new(firmware_semver: [u16; 3]) -> Self {
        Self {
            protocol_semver: SEMVER,
            firmware_semver,
        }
    }
}

/// Convenience helpers for MuxMsg to access device UUIDs when the message
/// references a single device. This returns the 6-byte device UUID where
/// applicable (for `DevicePacket` and `SendTo`). Other variants return `None`.
impl MuxMsg {
    /// If this message references a single device, return its 6-byte UUID.
    /// - `MuxMsg::DevicePacket` -> returns `Some(dev)`
    /// - `MuxMsg::SendTo` -> returns `Some(dev)`
    /// - otherwise -> `None`
    pub fn device_uuid(&self) -> Option<[u8; 6]> {
        match self {
            MuxMsg::DevicePacket(dp) => Some(dp.dev),
            MuxMsg::SendTo(s) => Some(s.dev),
            _ => None,
        }
    }

    /// If this message includes a snapshot of devices, return a reference to the vector.
    /// Useful for callers that want to examine the snapshot without matching the enum.
    pub fn devices_snapshot(&self) -> Option<&Vec<[u8; 6], MAX_DEVICES>> {
        match self {
            MuxMsg::DevicesSnapshot(devs) => Some(devs),
            _ => None,
        }
    }
}
