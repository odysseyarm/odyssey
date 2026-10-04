pub mod usb_mux;
pub mod device;

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "minicbor", derive(minicbor::Encode, minicbor::Decode, minicbor::CborLen))]
#[cfg_attr(feature = "minicbor", cbor(map))]
#[derive(Clone, Copy, Debug, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
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

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BondedDevice {
    pub uuid: [u8; 6],
    pub name: heapless::String<32>,
}

#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum AddBondError {
    Full,
    Failed,
}
