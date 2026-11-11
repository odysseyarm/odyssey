use odyssey_protocol_macros::protocol_type;

/// Since: Odyssey Desktop 0.6.0.
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Debug, Clone, Copy)]
pub enum PairingError {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Timeout,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Cancelled,
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug)]
pub enum ClearBondsError {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Failed,
}

/// Since: Odyssey Desktop 0.6.0.
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Copy, Debug)]
pub enum BondStoreError {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Full,
}

/// Since: Odyssey Desktop 0.6.0.
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Copy, Debug)]
pub struct StartPairing {
    /// Since: Odyssey Desktop 0.6.0.
    pub timeout_ms: u32,
}

/// Since: Odyssey Desktop 0.6.0.
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[derive(Clone, Debug)]
pub enum UsbMuxCtrlMsg {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    ReadVersion(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    ReadVersionResponse(crate::Version),

    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    ListBonds(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    ListBondsResponse(crate::List<super::BondedDevice, { crate::mux::MAX_DEVICES }>),

    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    ClearBonds(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    ClearBondsResponse(ClearBondsResult),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    BondStoreError(BondStoreError),

    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    StartPairing(StartPairing),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    StartPairingResponse(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    CancelPairing(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    PairingResult(PairingOutcome),

    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    AddBond(super::BondEntry),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    AddBondResponse(super::AddBondResult),

    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    UpdateBondName {
        /// Since: Odyssey Desktop 0.6.0.
        uuid: crate::mux::Uuid,
        /// Since: Odyssey Desktop 0.6.0.
        name: crate::DeviceName,
    },

    /// Resets the dongle into its vendor's Nordic DFU bootloader. No reply:
    /// the dongle leaves the bus and returns as the bootloader (PID 0x521F).
    ///
    /// Since: Odyssey Desktop 0.7.5.
    ///
    /// Wire baseline: device protocol 0.1.6.
    EnterBootloader(),
}

/// The result of a clear-bonds request. Serializes as the equivalent `Result`.
///
/// Since: Odyssey Desktop 0.6.0.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(
    feature = "serde",
    serde(
        from = "Result<(), ClearBondsError>",
        into = "Result<(), ClearBondsError>"
    )
)]
#[derive(Clone, Copy, Debug)]
pub enum ClearBondsResult {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Ok(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
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

/// The result of a pairing attempt. Serializes as the equivalent `Result`.
///
/// Since: Odyssey Desktop 0.6.0.
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(odyssey_protocol_macros::PostcardSchema))]
#[cfg_attr(
    feature = "serde",
    serde(
        from = "Result<super::BondedDevice, PairingError>",
        into = "Result<super::BondedDevice, PairingError>"
    )
)]
#[derive(Clone, Debug)]
pub enum PairingOutcome {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Ok(super::BondedDevice),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
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
