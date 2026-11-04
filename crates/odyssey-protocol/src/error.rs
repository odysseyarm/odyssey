use core::fmt;

use odyssey_protocol_macros::protocol_type;

use crate::PacketType;

#[protocol_type]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub enum Error {
    #[cfg_attr(feature = "minicbor", n(0))]
    UnexpectedEof {
        #[cfg_attr(feature = "minicbor", n(0))]
        packet_type: Option<PacketType>,
    },
    #[cfg_attr(feature = "minicbor", n(1))]
    UnrecognizedPacketId(#[cfg_attr(feature = "minicbor", n(0))] u8),
    #[cfg_attr(feature = "minicbor", n(2))]
    UnrecognizedPort,
    #[cfg_attr(feature = "minicbor", n(3))]
    UnrecognizedStreamUpdateAction(#[cfg_attr(feature = "minicbor", n(0))] u8),
    #[cfg_attr(feature = "minicbor", n(4))]
    InvalidBitPattern,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof { packet_type: None } => write!(f, "unexpected eof"),
            Self::UnexpectedEof {
                packet_type: Some(p),
            } => write!(f, "unexpected eof, packet id {p:?}"),
            Self::UnrecognizedPacketId(id) => write!(f, "unrecognized packet id {id}"),
            Self::UnrecognizedPort => write!(f, "unrecognized port"),
            Self::UnrecognizedStreamUpdateAction(n) => {
                write!(f, "unrecognized stream update action {n}")
            }
            Self::InvalidBitPattern => write!(f, "invalid bit pattern"),
        }
    }
}

impl core::error::Error for Error {}
