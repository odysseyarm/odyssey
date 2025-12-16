use crate::mux::Uuid;

const SEMVER: [u16; 3] = [0, 1, 0];

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Debug, Clone, Copy)]
pub enum PairingError {
    Timeout,
    Cancelled,
    NotBleMode,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TransportMode {
    Usb = 0,
    Ble = 1,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ClearBondsError {
    Failed,
}

#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Clone, Copy, Debug)]
pub struct StartPairing {
    pub timeout_ms: u32,
}

#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
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

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Clone, Debug)]
pub enum DeviceMsg {
    ReadVersion(),
    ReadVersionResponse(Version),

    ClearBond,
    ClearBondResponse(Result<(), ClearBondsError>),

    GetTransportMode,
    SetTransportMode(TransportMode),
    TransportModeStatus(TransportMode),

    StartPairing(StartPairing),
    StartPairingResponse,
    CancelPairing,
    PairingResult(Result<Uuid, PairingError>),
}
