use odyssey_protocol_macros::protocol_type;

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
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
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    NotBleMode,
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TransportMode {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(0))]
    Usb = 0,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    #[cfg_attr(feature = "minicbor", n(1))]
    Ble = 1,
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
#[protocol_type(no_cbor)]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug)]
pub struct StartPairing {
    /// Since: Odyssey Desktop 0.6.0.
    pub timeout_ms: u32,
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Debug)]
pub enum DeviceMsg {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    ReadProp(crate::PropKind),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    ReadPropResponse(crate::Props),

    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    ClearBond(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    ClearBondResponse(ClearBondResult),

    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    GetTransportMode(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    SetTransportMode(TransportMode),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    TransportModeStatus(TransportMode),

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
    ReadConfig(crate::ConfigKind),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    ReadConfigResponse(crate::GeneralConfig),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    WriteConfig(crate::GeneralConfig),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    WriteConfigAck(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    FlashSettings(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    FlashSettingsAck(),

    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Reboot(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    RebootAck(),

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
    SetDeviceName(crate::DeviceName),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    SetDeviceNameResponse(crate::SetDeviceNameResult),
}

/// The result of a clear-bond request. Serializes as the equivalent `Result`.
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
pub enum ClearBondResult {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Ok(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Err(ClearBondsError),
}

impl From<Result<(), ClearBondsError>> for ClearBondResult {
    fn from(value: Result<(), ClearBondsError>) -> Self {
        match value {
            Ok(()) => Self::Ok(),
            Err(e) => Self::Err(e),
        }
    }
}

impl From<ClearBondResult> for Result<(), ClearBondsError> {
    fn from(value: ClearBondResult) -> Self {
        match value {
            ClearBondResult::Ok() => Ok(()),
            ClearBondResult::Err(e) => Err(e),
        }
    }
}

/// The result of a pairing attempt. Serializes as the equivalent `Result`.
///
/// Since: Odyssey Desktop 0.6.0.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
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
