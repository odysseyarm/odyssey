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
#[cfg_attr(
    feature = "minicbor",
    derive(minicbor::Encode, minicbor::Decode, minicbor::CborLen)
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TransportMode {
    #[cfg_attr(feature = "minicbor", n(0))]
    Usb = 0,
    #[cfg_attr(feature = "minicbor", n(1))]
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

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Clone, Debug)]
pub enum DeviceMsg {
    ReadProp(crate::PropKind),
    ReadPropResponse(crate::Props),

    ClearBond(),
    ClearBondResponse(crate::Outcome<crate::Nothing, ClearBondsError>),

    GetTransportMode(),
    SetTransportMode(TransportMode),
    TransportModeStatus(TransportMode),

    StartPairing(StartPairing),
    StartPairingResponse(),
    CancelPairing(),
    PairingResult(crate::Outcome<super::BondedDevice, PairingError>),

    ReadConfig(crate::ConfigKind),
    ReadConfigResponse(crate::GeneralConfig),
    WriteConfig(crate::GeneralConfig),
    WriteConfigAck(),
    FlashSettings(),
    FlashSettingsAck(),

    Reboot(),
    RebootAck(),

    AddBond(super::BondEntry),
    AddBondResponse(crate::Outcome<crate::Nothing, super::AddBondError>),

    SetDeviceName(crate::DeviceName),
    SetDeviceNameResponse(crate::Outcome<crate::Nothing, crate::Nothing>),
}
