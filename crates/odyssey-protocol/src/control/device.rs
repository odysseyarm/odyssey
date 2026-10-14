use odyssey_protocol_macros::protocol_type;

#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Debug, Clone, Copy)]
pub enum PairingError {
    Timeout,
    Cancelled,
    NotBleMode,
}

#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TransportMode {
    #[cfg_attr(feature = "minicbor", n(0))]
    Usb = 0,
    #[cfg_attr(feature = "minicbor", n(1))]
    Ble = 1,
}

#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug)]
pub enum ClearBondsError {
    Failed,
}

#[protocol_type(no_cbor)]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug)]
pub struct StartPairing {
    pub timeout_ms: u32,
}

#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Debug)]
pub enum DeviceMsg {
    ReadProp(crate::PropKind),
    ReadPropResponse(crate::Props),

    ClearBond(),
    ClearBondResponse(ClearBondResult),

    GetTransportMode(),
    SetTransportMode(TransportMode),
    TransportModeStatus(TransportMode),

    StartPairing(StartPairing),
    StartPairingResponse(),
    CancelPairing(),
    PairingResult(PairingOutcome),

    ReadConfig(crate::ConfigKind),
    ReadConfigResponse(crate::GeneralConfig),
    WriteConfig(crate::GeneralConfig),
    WriteConfigAck(),
    FlashSettings(),
    FlashSettingsAck(),

    Reboot(),
    RebootAck(),

    AddBond(super::BondEntry),
    AddBondResponse(super::AddBondResult),

    SetDeviceName(crate::DeviceName),
    SetDeviceNameResponse(crate::SetDeviceNameResult),
}

/// The result of a clear-bond request. Serializes as the `Result` it replaces.
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
    Ok(),
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

/// The result of a pairing attempt. Serializes as the `Result` it replaces.
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
