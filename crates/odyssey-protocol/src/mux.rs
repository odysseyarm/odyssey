use crate::Packet;

pub const MAX_DEVICES: usize = 3;
pub type Uuid = [u8; 6];

/// Version of the mux wire format, reported by a dongle in
/// [`MuxMsg::ReadVersionResponse`]. Bump it when the mux messages change
/// shape; it moves independently of the crate version and of
/// [`crate::DEVICE_PROTOCOL`].
pub const MUX_PROTOCOL: [u16; 3] = [0, 4, 0];

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub enum MuxMsg {
    DevicesSnapshot(crate::List<Uuid, MAX_DEVICES>),
    DevicePacket(DevicePacket),
    RequestDevices(),
    SendTo(SendTo),
    ReadVersion(),
    ReadVersionResponse(Version),
    /// After this the dongle sends a `DevicesSnapshot` whenever a device
    /// connects or disconnects.
    SubscribeDeviceList(),
    UnsubscribeDeviceList(),
    AccessoryRequest(crate::accessory::Request),
    AccessoryCompletion(crate::accessory::Completion),
    AccessoryEvent(crate::accessory::Event),
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
            protocol_semver: MUX_PROTOCOL,
            firmware_semver,
        }
    }
}

impl MuxMsg {
    /// The device a `DevicePacket` or `SendTo` is addressed to; `None` for
    /// every other variant.
    pub fn device_uuid(&self) -> Option<[u8; 6]> {
        match self {
            Self::DevicePacket(dp) => Some(dp.dev),
            Self::SendTo(s) => Some(s.dev),
            _ => None,
        }
    }

    /// The device list carried by a `DevicesSnapshot`.
    pub fn devices_snapshot(&self) -> Option<&[Uuid]> {
        match self {
            Self::DevicesSnapshot(devs) => Some(devs),
            _ => None,
        }
    }
}
