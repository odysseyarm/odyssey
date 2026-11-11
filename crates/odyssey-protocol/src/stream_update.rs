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
        let &[packet_id, action, ref rest @ ..] = *bytes else {
            return Err(Error::UnexpectedEof {
                packet_type: Some(PacketType::StreamUpdate()),
            });
        };
        let stream_update = Self {
            packet_id: packet_id.try_into()?,
            action: action.try_into()?,
        };
        *bytes = rest;
        Ok(stream_update)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_update_does_not_consume_the_next_packet() {
        for bytes in [&[][..], &[0x09][..], &[0x7f, 0][..], &[0x09, 3][..]] {
            let mut cursor = bytes;
            assert!(StreamUpdate::parse(&mut cursor).is_err());
            assert_eq!(cursor, bytes);
        }
        let mut cursor = &[0x09, 0, 0xaa][..];
        let update = StreamUpdate::parse(&mut cursor).unwrap();
        assert!(matches!(update.packet_id, PacketType::LegacyObjectReport()));
        assert_eq!(update.action, StreamUpdateAction::Enable);
        assert_eq!(cursor, &[0xaa]);
    }
}
