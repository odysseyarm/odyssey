//! Supported accessory profiles over the vendor USB mux bulk endpoints.
//! Addresses use canonical MSB-first octets (as displayed), unlike raw HCI address bytes.
use crate::List;

pub const MIN_MUX_PROTOCOL: [u16; 3] = [0, 3, 0];
/// Older dongles cannot decode `Command::MarkerMode`; the host must not send it to them.
pub const MARKER_MODE_MUX_PROTOCOL: [u16; 3] = [0, 4, 0];
pub const ACCESSORY_CAPACITY: usize = 3;
pub const TOTAL_CAPACITY: usize = crate::mux::MAX_DEVICES + ACCESSORY_CAPACITY;
pub const MAX_NOTIFICATION: usize = 64;
pub const MAX_PENDING_REQUESTS: usize = 8;
pub const REQUEST_TIMEOUT_MILLIS: u64 = 30_000;
pub const NUS_SERVICE_UUID: [u8; 16] = 0x6e400001_b5a3_f393_e0a9_e50e24dcca9eu128.to_be_bytes();
pub const NUS_TX_UUID: [u8; 16] = 0x6e400003_b5a3_f393_e0a9_e50e24dcca9eu128.to_be_bytes();
pub const MANTIS_SERVICE_UUID: [u8; 16] = 0x6e400001_204d_616e_7469_732054656368u128.to_be_bytes();
pub const MANTIS_SHOT_UUID: [u8; 16] = 0x6e40000e_204d_616e_7469_732054656368u128.to_be_bytes();
pub const MANTIS_SHOT_MARKER: u8 = 1;

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    MarkerControlBoard,
    DryFireMag,
    MantisX,
}

impl Profile {
    pub const fn service_uuid(self) -> [u8; 16] {
        match self {
            Self::MarkerControlBoard => crate::marker_control::SERVICE_UUID,
            Self::DryFireMag => NUS_SERVICE_UUID,
            Self::MantisX => MANTIS_SERVICE_UUID,
        }
    }
    pub const fn characteristic_uuid(self) -> [u8; 16] {
        match self {
            Self::MarkerControlBoard => crate::marker_control::DEMAND_UUID,
            Self::DryFireMag => NUS_TX_UUID,
            Self::MantisX => MANTIS_SHOT_UUID,
        }
    }
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub total_capacity: u8,
    pub tracker_capacity: u8,
    pub accessory_capacity: u8,
    pub connected_trackers: u8,
    pub connected_accessories: u8,
    pub connecting_accessories: u8,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Capabilities(),
    Scan {
        enabled: bool,
    },
    Connect {
        address: [u8; 6],
        profile: Profile,
        enroll: bool,
    },
    Disconnect {
        address: [u8; 6],
    },
    MarkerDemand {
        address: [u8; 6],
        active: bool,
    },
    /// Acknowledged write of the marker board's Mode characteristic.
    MarkerMode {
        address: [u8; 6],
        mode: crate::marker_control::Mode,
    },
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Done(),
    Capabilities(Capabilities),
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    Busy,
    Capacity,
    NotConnected,
    UnknownPeer,
    InvalidProfile,
    Security,
    BondStorage,
    Timeout,
    Disconnected,
    Gatt,
    NotificationTooLarge,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "dongle firmware does not support accessories; update required",
            Self::Busy => "dongle accessory request queue full",
            Self::Capacity => "dongle accessory connection capacity exhausted",
            Self::NotConnected => "accessory not connected",
            Self::UnknownPeer => "accessory not discovered on selected dongle",
            Self::InvalidProfile => "accessory profile mismatch",
            Self::Security => "accessory secure enrollment or authentication failed",
            Self::BondStorage => "accessory bond persistence failed",
            Self::Timeout => "accessory BLE operation timed out",
            Self::Disconnected => "accessory disconnected",
            Self::Gatt => "accessory GATT operation failed",
            Self::NotificationTooLarge => "accessory notification exceeds wire limit",
        })
    }
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u32,
    pub command: Command,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    pub id: u32,
    pub result: Result<Reply, Error>,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Discovered {
        address: [u8; 6],
        profile: Profile,
        name: List<u8, 32>,
    },
    Connected {
        address: [u8; 6],
        profile: Profile,
    },
    Disconnected {
        address: [u8; 6],
    },
    Notification {
        address: [u8; 6],
        profile: Profile,
        data: List<u8, MAX_NOTIFICATION>,
    },
    Error {
        address: [u8; 6],
        error: Error,
    },
    PairingWindow {
        address: [u8; 6],
        seconds_remaining: u8,
    },
}
