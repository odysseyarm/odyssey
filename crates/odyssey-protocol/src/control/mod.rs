pub mod usb_mux;
pub mod device;

use odyssey_protocol_macros::protocol_type;

#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "minicbor", cbor(map))]
#[derive(Clone, Copy, Debug, Default)]
pub struct BondEntry {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub bd_addr: [u8; 6],
    #[cfg_attr(feature = "minicbor", n(1))]
    pub ltk: [u8; 16],
    #[cfg_attr(feature = "minicbor", n(2))]
    pub security_level: u8,
    #[cfg_attr(feature = "minicbor", n(3))]
    pub is_bonded: bool,
    #[cfg_attr(feature = "minicbor", n(4))]
    pub irk: Option<[u8; 16]>,
}

#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Debug)]
pub struct BondedDevice {
    pub uuid: [u8; 6],
    pub name: crate::DeviceName,
}

#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug)]
pub enum AddBondError {
    Full,
    Failed,
}

/// The result of an add-bond request. Serializes as the `Result` it replaces.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(
    feature = "serde",
    serde(from = "Result<(), AddBondError>", into = "Result<(), AddBondError>")
)]
#[derive(Clone, Copy, Debug)]
pub enum AddBondResult {
    Ok(),
    Err(AddBondError),
}

impl From<Result<(), AddBondError>> for AddBondResult {
    fn from(value: Result<(), AddBondError>) -> Self {
        match value {
            Ok(()) => Self::Ok(),
            Err(e) => Self::Err(e),
        }
    }
}

impl From<AddBondResult> for Result<(), AddBondError> {
    fn from(value: AddBondResult) -> Self {
        match value {
            AddBondResult::Ok() => Ok(()),
            AddBondResult::Err(e) => Err(e),
        }
    }
}
