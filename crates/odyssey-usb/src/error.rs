use std::time::Duration;

use odyssey_protocol::PacketType;
use odyssey_protocol::control::AddBondError;
use odyssey_protocol::control::device::{PairingError, TransportMode};

/// Everything that can go wrong talking to a vision module or a dongle.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Opening the device or claiming one of its interfaces failed.
    #[error("opening the USB device failed")]
    Open(#[from] nusb::Error),
    /// The device has no vendor-specific (class 0xFF) interface.
    #[error("the device has no vendor-specific interface")]
    NoVendorInterface,
    /// A bulk endpoint the protocol needs is missing from the interface.
    #[error("bulk endpoint {address:#04x} is not available")]
    Endpoint {
        address: u8,
        #[source]
        source: nusb::Error,
    },
    /// No interface of the dongle has the mux protocol's endpoints.
    #[error("the dongle has no interface with the mux endpoints")]
    NoMuxInterface,
    /// The device was opened without its control interface, so control
    /// requests (pairing, transport mode, bonds) are impossible.
    #[error("no control interface; the device is reached through a dongle")]
    NoControlInterface,
    /// A control transfer failed on the bus.
    #[error("USB control transfer failed")]
    Transfer(#[from] nusb::transfer::TransferError),
    /// A message could not be encoded for, or decoded from, the device.
    #[error("control message encoding failed")]
    Codec(#[from] postcard::Error),
    /// The link to the device is gone: the device was unplugged, the dongle
    /// lost it, or the transport was shut down. Retrying will not help.
    #[error("the link to the device is closed")]
    Closed,
    /// Every request id is waiting on a reply. Transient.
    #[error("too many requests in flight")]
    Busy,
    /// No reply arrived in time.
    #[error("no reply within {0:?}")]
    Timeout(Duration),
    /// The request was resent and every attempt timed out.
    #[error("no reply after {attempts} attempts of {per_attempt:?}")]
    NoResponse {
        attempts: usize,
        per_attempt: Duration,
    },
    /// The device answered with a message the request does not expect; the
    /// `Debug` form of that message.
    #[error("unexpected reply: {0}")]
    UnexpectedReply(String),
    /// The device is not in the transport mode the operation needs.
    #[error("the device is in {actual:?} mode, not {expected:?}")]
    WrongTransportMode {
        expected: TransportMode,
        actual: TransportMode,
    },
    /// A stream of this type is already open on the device.
    #[error("a {0:?} stream is already open")]
    StreamOpen(PacketType),
    /// An argument is outside what the protocol can carry.
    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),
    /// The device carried out the request and reported that it failed.
    #[error("the device refused: {0}")]
    Refused(&'static str),
    /// A pairing attempt ended without a bond.
    #[error("pairing failed: {0:?}")]
    Pairing(PairingError),
    /// The device or dongle would not store the bond.
    #[error("the bond was not stored: {0:?}")]
    BondRejected(AddBondError),
    /// A camera model read from the device is not a valid intrinsics matrix.
    #[error("the device holds an invalid camera model: {0}")]
    InvalidCameraModel(String),
    /// Image data is supported only on a direct USB data link.
    #[error("live images require a direct USB connection")]
    ImageUsbRequired,
    /// Firmware explicitly refuses capture, or has no image command protocol.
    #[error("this firmware does not support live image capture")]
    ImageUnsupported,
    /// A truthful firmware status rejected or ended capture.
    #[error("image capture status: {0:?}")]
    ImageStatus(odyssey_protocol::image::ImageStatusCode),
    /// A malformed frame was discarded without exposing partial pixels.
    #[error("invalid image frame: {0}")]
    InvalidImage(&'static str),
}

impl Error {
    /// An unexpected reply, recorded by its `Debug` form.
    pub(crate) fn unexpected(reply: &impl std::fmt::Debug) -> Self {
        Self::UnexpectedReply(format!("{reply:?}"))
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
