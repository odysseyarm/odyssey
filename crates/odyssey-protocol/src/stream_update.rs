use odyssey_protocol_macros::protocol_type;

use crate::{Error, PacketType};

#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[repr(u8)]
#[derive(Clone, Copy, Debug, enumn::N, PartialEq)]
pub enum StreamUpdateAction {
    #[cfg_attr(feature = "minicbor", n(0))]
    Enable,
    #[cfg_attr(feature = "minicbor", n(1))]
    Disable,
    #[cfg_attr(feature = "minicbor", n(2))]
    DisableAll,
}

impl TryFrom<u8> for StreamUpdateAction {
    type Error = Error;
    fn try_from(n: u8) -> Result<Self, Self::Error> {
        Self::n(n).ok_or(Error::UnrecognizedStreamUpdateAction(n))
    }
}

#[protocol_type]
#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug)]
pub struct StreamUpdate {
    #[cfg_attr(feature = "minicbor", n(0))]
    pub packet_id: PacketType,
    #[cfg_attr(feature = "minicbor", n(1))]
    pub action: StreamUpdateAction,
}

impl StreamUpdate {
    pub fn parse(bytes: &mut &[u8]) -> Result<Self, Error> {
        let stream_update = Self {
            packet_id: bytes[0].try_into()?,
            action: bytes[1].try_into()?,
        };
        *bytes = &bytes[2..];
        Ok(stream_update)
    }
}
