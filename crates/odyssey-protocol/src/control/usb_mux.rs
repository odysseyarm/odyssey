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
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
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

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub enum UsbMuxCtrlMsg {
    ReadVersion(),
    ReadVersionResponse(crate::Version),

    ListBonds(),
    ListBondsResponse(crate::List<super::BondedDevice, { crate::mux::MAX_DEVICES }>),

    ClearBonds(),
    ClearBondsResponse(ClearBondsResult),
    BondStoreError(BondStoreError),

    StartPairing(StartPairing),
    StartPairingResponse(),
    CancelPairing(),
    PairingResult(PairingOutcome),

    AddBond(super::BondEntry),
    AddBondResponse(super::AddBondResult),

    UpdateBondName {
        uuid: crate::mux::Uuid,
        name: crate::DeviceName,
    },
}

/// The result of a clear-bonds request. Serializes as the `Result` it replaces.
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(
        from = "Result<(), ClearBondsError>",
        into = "Result<(), ClearBondsError>"
    )
)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Clone, Copy, Debug)]
pub enum ClearBondsResult {
    Ok(),
    Err(ClearBondsError),
}

impl From<Result<(), ClearBondsError>> for ClearBondsResult {
    fn from(value: Result<(), ClearBondsError>) -> Self {
        match value {
            Ok(()) => Self::Ok(),
            Err(e) => Self::Err(e),
        }
    }
}

impl From<ClearBondsResult> for Result<(), ClearBondsError> {
    fn from(value: ClearBondsResult) -> Self {
        match value {
            ClearBondsResult::Ok() => Ok(()),
            ClearBondsResult::Err(e) => Err(e),
        }
    }
}

/// The result of a pairing attempt. Serializes as the `Result` it replaces.
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(
        from = "Result<super::BondedDevice, PairingError>",
        into = "Result<super::BondedDevice, PairingError>"
    )
)]
#[derive(Clone, Debug)]
pub enum PairingOutcome {
    Ok(super::BondedDevice),
    Err(PairingError),
}

impl From<Result<super::BondedDevice, PairingError>> for PairingOutcome {
    fn from(value: Result<super::BondedDevice, PairingError>) -> Self {
        match value {
            Ok(d) => Self::Ok(d),
            Err(e) => Self::Err(e),
        }
    }
}

impl From<PairingOutcome> for Result<super::BondedDevice, PairingError> {
    fn from(value: PairingOutcome) -> Self {
        match value {
            PairingOutcome::Ok(d) => Ok(d),
            PairingOutcome::Err(e) => Err(e),
        }
    }
}
