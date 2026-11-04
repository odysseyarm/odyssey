use odyssey_protocol_macros::protocol_type;

use crate::Error;

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Register {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub port: Port,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub bank: u8,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub address: u8,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WriteRegister {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub port: Port,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub bank: u8,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub address: u8,
    #[cfg_attr(feature = "minicbor", n(3))]
    pub data: u8,
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadRegisterResponse {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub bank: u8,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub address: u8,
    #[cfg_attr(feature = "minicbor", n(2))]
    pub data: u8,
}

#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port {
    #[cfg_attr(feature = "minicbor", n(0))]
    Nf,
    #[cfg_attr(feature = "minicbor", n(1))]
    Wf,
}
impl TryFrom<u8> for Port {
    type Error = Error;
    fn try_from(n: u8) -> Result<Self, Self::Error> {
        match n {
            0 => Ok(Self::Nf),
            1 => Ok(Self::Wf),
            _ => Err(Error::UnrecognizedPort),
        }
    }
}
