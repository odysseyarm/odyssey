use crate::mux::Uuid;

const SEMVER: [u16; 3] = [0, 1, 0];

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy)]
pub enum PairingError {
    Timeout,
    Cancelled,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub enum ClearBondsError {
    Failed,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub enum BondStoreError {
    Full,
}

#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct StartPairing {
    pub timeout_ms: u32,
}

#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct UsbMuxVersion {
    pub protocol_semver: [u16; 3],
    pub firmware_semver: [u16; 3],
}

impl UsbMuxVersion {
    pub fn new(firmware_semver: [u16; 3]) -> Self {
        Self {
            protocol_semver: SEMVER,
            firmware_semver,
        }
    }
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub enum UsbMuxCtrlMsg {
    ReadVersion(),
    ReadVersionResponse(UsbMuxVersion),

    ClearBonds,
    ClearBondsResponse(Result<(), ClearBondsError>),
    BondStoreError(BondStoreError),

    StartPairing(StartPairing),
    StartPairingResponse,
    CancelPairing,
    PairingResult(Result<Uuid, PairingError>),
}
