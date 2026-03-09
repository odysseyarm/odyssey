pub mod usb_mux;
pub mod device;

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BondEntry {
    pub bd_addr: [u8; 6],
    pub ltk: [u8; 16],
    pub security_level: u8,
    pub is_bonded: bool,
    pub irk: Option<[u8; 16]>,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum AddBondError {
    Full,
    Failed,
}
