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

    ListBonds,
    ListBondsResponse(heapless::Vec<super::BondedDevice, { crate::mux::MAX_DEVICES }>),

    ClearBonds,
    ClearBondsResponse(Result<(), ClearBondsError>),
    BondStoreError(BondStoreError),

    StartPairing(StartPairing),
    StartPairingResponse,
    CancelPairing,
    PairingResult(Result<super::BondedDevice, PairingError>),

    AddBond(super::BondEntry),
    AddBondResponse(Result<(), super::AddBondError>),

    UpdateBondName { uuid: crate::mux::Uuid, name: heapless::String<32> },
}
