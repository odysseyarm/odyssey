use crate::Packet;

/// Since: Odyssey Desktop 0.6.0.
pub const MAX_DEVICES: usize = 3;
/// Since: Odyssey Desktop 0.6.0.
pub type Uuid = [u8; 6];

/// Version of the mux wire format, reported by a dongle in
/// [`MuxMsg::ReadVersionResponse`]. Bump it when mux messages change shape.
/// It moves independently of the crate version and [`crate::DEVICE_PROTOCOL`].
///
/// Since: Odyssey Desktop 0.6.0.
pub const MUX_PROTOCOL: [u16; 3] = [0, 5, 0];

/// Since: Odyssey Desktop 0.6.0.
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Debug)]
pub enum MuxMsg {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: mux protocol 0.2.0.
    DevicesSnapshot(crate::List<Uuid, MAX_DEVICES>),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: mux protocol 0.2.0.
    DevicePacket(DevicePacket),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: mux protocol 0.2.0.
    RequestDevices(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: mux protocol 0.2.0.
    SendTo(SendTo),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: mux protocol 0.2.0.
    ReadVersion(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: mux protocol 0.2.0.
    ReadVersionResponse(Version),
    /// After this the dongle sends a `DevicesSnapshot` whenever a device
    /// connects or disconnects.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: mux protocol 0.2.0.
    SubscribeDeviceList(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: mux protocol 0.2.0.
    UnsubscribeDeviceList(),
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    AccessoryRequest(crate::accessory::Request),
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    AccessoryCompletion(crate::accessory::Completion),
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    AccessoryEvent(crate::accessory::Event),
}

/// Since: Odyssey Desktop 0.6.0.
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Debug)]
pub struct DevicePacket {
    /// Since: Odyssey Desktop 0.6.0.
    pub dev: Uuid,
    /// Since: Odyssey Desktop 0.6.0.
    pub pkt: Packet,
}

/// Since: Odyssey Desktop 0.6.0.
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Debug)]
pub struct SendTo {
    /// Since: Odyssey Desktop 0.6.0.
    pub dev: Uuid,
    /// Since: Odyssey Desktop 0.6.0.
    pub pkt: Packet,
}

/// Since: Odyssey Desktop 0.6.0.
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Copy, Debug)]
pub struct Version {
    /// Since: Odyssey Desktop 0.6.0.
    pub protocol_semver: [u16; 3],
    /// Since: Odyssey Desktop 0.6.0.
    pub firmware_semver: [u32; 3],
}

impl Version {
    /// Since: Odyssey Desktop 0.6.0.
    pub fn new(firmware_semver: [u32; 3]) -> Self {
        Self {
            protocol_semver: MUX_PROTOCOL,
            firmware_semver,
        }
    }
}

impl MuxMsg {
    /// The device a `DevicePacket` or `SendTo` is addressed to. `None` for
    /// every other variant.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn device_uuid(&self) -> Option<[u8; 6]> {
        match self {
            Self::DevicePacket(dp) => Some(dp.dev),
            Self::SendTo(s) => Some(s.dev),
            _ => None,
        }
    }

    /// The device list carried by a `DevicesSnapshot`.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn devices_snapshot(&self) -> Option<&[Uuid]> {
        match self {
            Self::DevicesSnapshot(devs) => Some(devs),
            _ => None,
        }
    }
}
