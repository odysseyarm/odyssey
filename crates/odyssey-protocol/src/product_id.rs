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
    const ALL: [Self; 4] = [Self::AtsVm, Self::AtsLite, Self::AtsPro, Self::Mux];

    pub fn from_u16(value: u16) -> Option<Self> {
        Self::ALL.into_iter().find(|id| *id as u16 == value)
    }

    pub fn supports_control_endpoint(&self) -> bool {
        matches!(self, Self::AtsLite | Self::Mux)
    }
}
