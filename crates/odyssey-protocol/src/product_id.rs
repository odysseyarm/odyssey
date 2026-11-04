use odyssey_protocol_macros::protocol_type;

#[protocol_type(no_cbor)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum ProductId {
    AtsVm = 0x520f,
    AtsLite = 0x5210,
    AtsPro = 0x5211,
    Mux = 0x5212,
}

impl ProductId {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0x520f => Some(Self::AtsVm),
            0x5210 => Some(Self::AtsLite),
            0x5211 => Some(Self::AtsPro),
            0x5212 => Some(Self::Mux),
            _ => None,
        }
    }

    pub fn supports_control_endpoint(&self) -> bool {
        matches!(self, Self::AtsLite | Self::Mux)
    }
}
