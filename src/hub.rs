use heapless::Vec;

use crate::Packet;

pub const MAX_DEVICES: usize = 3;
type Uuid = [u8; 6];

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub enum HubMsg {
    ClearBonds,
    ClearBondsResponse(ClearBondsResult),
    BondStoreError(BondStoreError),

    DevicesSnapshot(Vec<Uuid, MAX_DEVICES>),
    DevicePacket(DevicePacket),
    RequestDevices,
    SendTo(SendTo),
    ReadVersion(),
    ReadVersionResponse(Version),
    StartPairing(StartPairing),
    PairingStarted,
    PairingTimeout,
    CancelPairing,
    PairingCancelled,
    PairingResult(Uuid),
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
        const PROTO_MAJOR: u16 =
            match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_MAJOR"), 10) {
                Ok(v) => v,
                Err(_) => panic!("Invalid CARGO_PKG_VERSION_MAJOR"),
            };
        const PROTO_MINOR: u16 =
            match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_MINOR"), 10) {
                Ok(v) => v,
                Err(_) => panic!("Invalid CARGO_PKG_VERSION_MINOR"),
            };
        const PROTO_PATCH: u16 =
            match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_PATCH"), 10) {
                Ok(v) => v,
                Err(_) => panic!("Invalid CARGO_PKG_VERSION_PATCH"),
            };
        Self {
            protocol_semver: [PROTO_MAJOR, PROTO_MINOR, PROTO_PATCH],
            firmware_semver,
        }
    }
}

#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct StartPairing {
    pub timeout_ms: u32,
}

/// Convenience helpers for HubMsg to access device UUIDs when the message
/// references a single device. This returns the 6-byte device UUID where
/// applicable (for `DevicePacket` and `SendTo`). Other variants return `None`.
impl HubMsg {
    /// If this message references a single device, return its 6-byte UUID.
    /// - `HubMsg::DevicePacket` -> returns `Some(dev)`
    /// - `HubMsg::SendTo` -> returns `Some(dev)`
    /// - otherwise -> `None`
    pub fn device_uuid(&self) -> Option<[u8; 6]> {
        match self {
            HubMsg::DevicePacket(dp) => Some(dp.dev),
            HubMsg::SendTo(s) => Some(s.dev),
            _ => None,
        }
    }

    /// If this message includes a snapshot of devices, return a reference to the vector.
    /// Useful for callers that want to examine the snapshot without matching the enum.
    pub fn devices_snapshot(&self) -> Option<&Vec<[u8; 6], MAX_DEVICES>> {
        match self {
            HubMsg::DevicesSnapshot(devs) => Some(devs),
            _ => None,
        }
    }
}

// Added bonds management messages
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub enum ClearBondsResult {
    Success,
}

#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub enum BondStoreError {
    Full,
}
