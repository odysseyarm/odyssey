//! Supported accessory profiles over the vendor USB mux bulk endpoints.
//! Addresses use canonical MSB-first octets (as displayed), unlike raw HCI address bytes.
use crate::List;

/// Since: Odyssey Desktop 0.7.4.
pub const MIN_MUX_PROTOCOL: [u16; 3] = [0, 3, 0];
/// Older dongles cannot decode `Command::MarkerMode`, so the host must not send it to them.
///
/// Since: Odyssey Desktop 0.7.4.
pub const MARKER_MODE_MUX_PROTOCOL: [u16; 3] = [0, 4, 0];
/// Since: Odyssey Desktop 0.7.4.
pub const ACCESSORY_CAPACITY: usize = 3;
/// Since: Odyssey Desktop 0.7.4.
pub const TOTAL_CAPACITY: usize = crate::mux::MAX_DEVICES + ACCESSORY_CAPACITY;
/// Since: Odyssey Desktop 0.7.4.
pub const MAX_NOTIFICATION: usize = 64;
/// Since: Odyssey Desktop 0.7.4.
pub const MAX_PENDING_REQUESTS: usize = 8;
/// Since: Odyssey Desktop 0.7.4.
pub const REQUEST_TIMEOUT_MILLIS: u64 = 30_000;
/// Since: Odyssey Desktop 0.7.4.
pub const NUS_SERVICE_UUID: [u8; 16] = 0x6e400001_b5a3_f393_e0a9_e50e24dcca9eu128.to_be_bytes();
/// Since: Odyssey Desktop 0.7.4.
pub const NUS_TX_UUID: [u8; 16] = 0x6e400003_b5a3_f393_e0a9_e50e24dcca9eu128.to_be_bytes();
/// Since: Odyssey Desktop 0.7.4.
pub const MANTIS_SERVICE_UUID: [u8; 16] = 0x6e400001_204d_616e_7469_732054656368u128.to_be_bytes();
/// Since: Odyssey Desktop 0.7.4.
pub const MANTIS_SHOT_UUID: [u8; 16] = 0x6e40000e_204d_616e_7469_732054656368u128.to_be_bytes();
/// Since: Odyssey Desktop 0.7.4.
pub const MANTIS_SHOT_MARKER: u8 = 1;

/// Since: Odyssey Desktop 0.7.4.
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    MarkerControlBoard,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    DryFireMag,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    MantisX,
}

impl Profile {
    /// Since: Odyssey Desktop 0.7.4.
    pub const fn service_uuid(self) -> [u8; 16] {
        match self {
            Self::MarkerControlBoard => crate::marker_control::SERVICE_UUID,
            Self::DryFireMag => NUS_SERVICE_UUID,
            Self::MantisX => MANTIS_SERVICE_UUID,
        }
    }
    /// Since: Odyssey Desktop 0.7.4.
    pub const fn characteristic_uuid(self) -> [u8; 16] {
        match self {
            Self::MarkerControlBoard => crate::marker_control::DEMAND_UUID,
            Self::DryFireMag => NUS_TX_UUID,
            Self::MantisX => MANTIS_SHOT_UUID,
        }
    }
}

/// Since: Odyssey Desktop 0.7.4.
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// Since: Odyssey Desktop 0.7.4.
    pub total_capacity: u8,
    /// Since: Odyssey Desktop 0.7.4.
    pub tracker_capacity: u8,
    /// Since: Odyssey Desktop 0.7.4.
    pub accessory_capacity: u8,
    /// Since: Odyssey Desktop 0.7.4.
    pub connected_trackers: u8,
    /// Since: Odyssey Desktop 0.7.4.
    pub connected_accessories: u8,
    /// Since: Odyssey Desktop 0.7.4.
    pub connecting_accessories: u8,
}

/// Since: Odyssey Desktop 0.7.4.
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Capabilities(),
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Scan {
        /// Since: Odyssey Desktop 0.7.4.
        enabled: bool,
    },
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Connect {
        /// Since: Odyssey Desktop 0.7.4.
        address: [u8; 6],
        /// Since: Odyssey Desktop 0.7.4.
        profile: Profile,
        /// Since: Odyssey Desktop 0.7.4.
        enroll: bool,
    },
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Disconnect {
        /// Since: Odyssey Desktop 0.7.4.
        address: [u8; 6],
    },
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    MarkerDemand {
        /// Since: Odyssey Desktop 0.7.4.
        address: [u8; 6],
        /// Since: Odyssey Desktop 0.7.4.
        active: bool,
    },
    /// Acknowledged write of the marker board's Mode characteristic.
    ///
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    MarkerMode {
        /// Since: Odyssey Desktop 0.7.4.
        address: [u8; 6],
        /// Since: Odyssey Desktop 0.7.4.
        mode: crate::marker_control::Mode,
    },
    /// Acknowledged write carrying one raw OTA request frame.
    ///
    /// Since: Unreleased.
    ///
    /// Wire baseline: mux protocol 0.6.0.
    MarkerOta {
        /// Since: Unreleased.
        address: [u8; 6],
        /// Since: Unreleased.
        frame: crate::VendorData,
    },
    /// Reads the marker board's OTA status.
    ///
    /// Since: Unreleased.
    ///
    /// Wire baseline: mux protocol 0.6.0.
    MarkerOtaStatus {
        /// Since: Unreleased.
        address: [u8; 6],
    },
}

/// Since: Odyssey Desktop 0.7.4.
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Done(),
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Capabilities(Capabilities),
    /// Raw 22-byte marker OTA status.
    ///
    /// Since: Unreleased.
    ///
    /// Wire baseline: mux protocol 0.6.0.
    MarkerOtaStatus([u8; 22]),
}

/// Since: Odyssey Desktop 0.7.4.
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Unsupported,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Busy,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Capacity,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    NotConnected,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    UnknownPeer,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    InvalidProfile,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Security,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    BondStorage,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Timeout,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Disconnected,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Gatt,
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
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

/// Since: Odyssey Desktop 0.7.4.
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    /// Since: Odyssey Desktop 0.7.4.
    pub id: u32,
    /// Since: Odyssey Desktop 0.7.4.
    pub command: Command,
}

/// Since: Odyssey Desktop 0.7.4.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    /// Since: Odyssey Desktop 0.7.4.
    pub id: u32,
    /// Since: Odyssey Desktop 0.7.4.
    pub result: Result<Reply, Error>,
}

/// Since: Odyssey Desktop 0.7.4.
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Discovered {
        /// Since: Odyssey Desktop 0.7.4.
        address: [u8; 6],
        /// Since: Odyssey Desktop 0.7.4.
        profile: Profile,
        /// Since: Odyssey Desktop 0.7.4.
        name: List<u8, 32>,
    },
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Connected {
        /// Since: Odyssey Desktop 0.7.4.
        address: [u8; 6],
        /// Since: Odyssey Desktop 0.7.4.
        profile: Profile,
    },
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Disconnected {
        /// Since: Odyssey Desktop 0.7.4.
        address: [u8; 6],
    },
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Notification {
        /// Since: Odyssey Desktop 0.7.4.
        address: [u8; 6],
        /// Since: Odyssey Desktop 0.7.4.
        profile: Profile,
        /// Since: Odyssey Desktop 0.7.4.
        data: List<u8, MAX_NOTIFICATION>,
    },
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    Error {
        /// Since: Odyssey Desktop 0.7.4.
        address: [u8; 6],
        /// Since: Odyssey Desktop 0.7.4.
        error: Error,
    },
    /// Since: Odyssey Desktop 0.7.4.
    ///
    /// Wire baseline: mux protocol 0.4.0.
    PairingWindow {
        /// Since: Odyssey Desktop 0.7.4.
        address: [u8; 6],
        /// Since: Odyssey Desktop 0.7.4.
        seconds_remaining: u8,
    },
}
