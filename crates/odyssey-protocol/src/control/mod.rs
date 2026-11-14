/// Since: Odyssey Desktop 0.6.0.
pub mod device;
/// Since: Odyssey Desktop 0.6.0.
pub mod usb_mux;

use odyssey_protocol_macros::protocol_type;

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "minicbor", cbor(map))]
#[derive(Clone, Copy, Debug, Default)]
pub struct BondEntry {
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub bd_addr: [u8; 6],
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub ltk: [u8; 16],
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(2))]
    pub security_level: u8,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(3))]
    pub is_bonded: bool,
    /// Since: Odyssey Desktop 0.6.0.
    #[cfg_attr(feature = "minicbor", n(4))]
    pub irk: Option<[u8; 16]>,
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Debug)]
pub struct BondedDevice {
    /// Since: Odyssey Desktop 0.6.0.
    pub uuid: [u8; 6],
    /// Since: Odyssey Desktop 0.6.0.
    pub name: crate::DeviceName,
}

/// Since: Odyssey Desktop 0.6.0.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug)]
pub enum AddBondError {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Full,
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Failed,
}

/// The result of an add-bond request. Serializes as the equivalent `Result`.
///
/// Since: Odyssey Desktop 0.6.0.
#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[cfg_attr(
    feature = "serde",
    serde(from = "Result<(), AddBondError>", into = "Result<(), AddBondError>")
)]
#[derive(Clone, Copy, Debug)]
pub enum AddBondResult {
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
    Ok(),
    /// Since: Odyssey Desktop 0.6.0.
    ///
    /// Wire baseline: device protocol 0.1.4.
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
