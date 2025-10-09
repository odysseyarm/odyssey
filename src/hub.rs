use heapless::Vec;

use crate::Packet;

pub const MAX_DEVICES: usize = 7;
type Uuid = [u8; 6];

#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub enum HubMsg {
    DevicesSnapshot(Vec<Uuid, MAX_DEVICES>),
    DevicePacket(DevicePacket),
    RequestDevices,
    SendTo(SendTo),
    ReadVersion(),
    ReadVersionResponse(Version),
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
