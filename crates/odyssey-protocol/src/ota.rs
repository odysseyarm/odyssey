//! Wire format of the radio OTA protocol: request frames, the status reply, and the CRC-32 that
//! guards each staged image.
//!
//! The device side is `Receiver` in `firmware/better-ota/src/receiver.rs`; the host side is
//! `crates/odyssey-hub-server/src/firmware/ota.rs`. Both only see byte frames, so the same codec
//! serves every transport. `firmware/better-ota/src/lib.rs` describes the whole flow.
//!
//! Every request gets a [`Status`] back. Requests are one frame of at most [`MAX_FRAME`] bytes:
//!
//! | Byte 0 | Request          | Remaining bytes (little endian)           | Frame length |
//! |--------|------------------|-------------------------------------------|--------------|
//! | 0      | [`Request::Query`]  | none                                   | 1            |
//! | 1      | [`Request::Begin`]  | image `u8`, size `u32`, CRC-32 `u32`   | 10           |
//! | 2      | [`Request::Data`]   | offset `u32`, then image bytes         | 6 to 98      |
//! | 3      | [`Request::Commit`] | none                                   | 1            |
//! | 4      | [`Request::Abort`]  | none                                   | 1            |
//!
//! The status reply is [`STATUS_LEN`] bytes. See [`Status::encode`] for the layout.

/// Tag of the vendor packet that carries OTA frames on the L2CAP packet link. The marker hint uses
/// [`crate::marker_hint::VENDOR_TAG`].
///
/// Since: Unreleased.
pub const VENDOR_TAG: u8 = 0xfe;
/// Largest request frame: one opcode byte, a 4-byte offset and up to 93 data bytes. Equals
/// [`crate::VendorData::CAPACITY`], so a frame fits one vendor packet.
///
/// Since: Unreleased.
pub const MAX_FRAME: usize = 98;
/// Length of an encoded [`Status`].
///
/// Since: Unreleased.
pub const STATUS_LEN: usize = 22;
/// First byte of every status. A device with a different value is not speaking this protocol.
///
/// Since: Unreleased.
pub const PROTOCOL_VERSION: u8 = 1;
/// Oldest dongle mux protocol that relays the accessory commands `MarkerOta` and
/// `MarkerOtaStatus` to a marker board.
///
/// Since: Unreleased.
pub const MARKER_OTA_MUX_PROTOCOL: [u16; 3] = [0, 6, 0];

/// Image a transfer targets.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Image {
    /// Application firmware. The device writes it to the slot it is not running from.
    Application = 0,
    /// The nRF5340 network core firmware.
    NetCore = 1,
}

impl Image {
    /// Bit of this image in [`Status::supported`] and [`Status::staged`].
    ///
    /// Since: Unreleased.
    pub const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// Result code in a [`Status`], also the error type of the request codec and the receiver.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Code {
    /// The last request succeeded.
    Ok = 0,
    /// The frame is malformed.
    InvalidFrame = 1,
    /// A transfer or staged image already exists for this request.
    Busy = 2,
    /// The device cannot take this image or operation.
    Unsupported = 3,
    /// A length is out of range for the frame, the image or the flash slot.
    Size = 4,
    /// A data frame does not start where the previous one ended.
    Offset = 5,
    /// A data frame breaks the 4-byte flash write alignment.
    Alignment = 6,
    /// A flash read, write, erase or commit failed. The receiver drops its staged state.
    Flash = 7,
    /// The staged image does not match the CRC-32 from `Begin`. The receiver drops its staged state.
    Checksum = 8,
    /// The staged image failed the target's validity check (reset and stack vectors). The
    /// receiver drops its staged state.
    Image = 9,
    /// The request is not valid in the receiver's current state.
    State = 10,
}

impl core::fmt::Display for Code {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "OTA error {}", *self as u8)
    }
}

impl core::error::Error for Code {}

const OP_QUERY: u8 = 0;
const OP_BEGIN: u8 = 1;
const OP_DATA: u8 = 2;
const OP_COMMIT: u8 = 3;
const OP_ABORT: u8 = 4;
/// Opcode, image, size, CRC-32.
const BEGIN_LEN: usize = 10;
/// Opcode and offset.
const DATA_HEADER_LEN: usize = 5;

/// One request from the host.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Request<'a> {
    /// Reads the status without changing state. Clears the last error code.
    Query,
    /// Starts a transfer of `size` bytes whose CRC-32 is `crc32`.
    Begin { image: Image, size: u32, crc32: u32 },
    /// Next chunk of the transfer. `offset` must equal the bytes received so far.
    Data { offset: u32, data: &'a [u8] },
    /// Activates every fully received image on the next reset.
    Commit,
    /// Drops the transfer in progress and everything staged.
    Abort,
}

impl<'a> Request<'a> {
    /// Decodes one request frame.
    ///
    /// # Errors
    ///
    /// Returns [`Code::Image`] for an unknown image, [`Code::Size`] for a data
    /// frame over [`MAX_FRAME`], and [`Code::InvalidFrame`] for any other
    /// malformed frame.
    ///
    /// Since: Unreleased.
    pub fn decode(frame: &'a [u8]) -> Result<Self, Code> {
        match frame.first() {
            Some(&OP_QUERY) if frame.len() == 1 => Ok(Self::Query),
            Some(&OP_BEGIN) if frame.len() == BEGIN_LEN => {
                let image = match frame[1] {
                    0 => Image::Application,
                    1 => Image::NetCore,
                    _ => return Err(Code::Image),
                };
                Ok(Self::Begin {
                    image,
                    size: le_u32(&frame[2..6])?,
                    crc32: le_u32(&frame[6..10])?,
                })
            }
            Some(&OP_DATA) if (DATA_HEADER_LEN + 1..=MAX_FRAME).contains(&frame.len()) => {
                Ok(Self::Data {
                    offset: le_u32(&frame[1..DATA_HEADER_LEN])?,
                    data: &frame[DATA_HEADER_LEN..],
                })
            }
            Some(&OP_COMMIT) if frame.len() == 1 => Ok(Self::Commit),
            Some(&OP_ABORT) if frame.len() == 1 => Ok(Self::Abort),
            // A data frame with no payload is malformed; one over MAX_FRAME is only too big.
            Some(&OP_DATA) if frame.len() <= DATA_HEADER_LEN => Err(Code::InvalidFrame),
            Some(&OP_DATA) => Err(Code::Size),
            _ => Err(Code::InvalidFrame),
        }
    }

    /// Encodes the request into `output` and returns the frame length.
    ///
    /// # Errors
    ///
    /// Returns [`Code::Size`] if `output` is too short or a data frame is empty
    /// or over [`MAX_FRAME`].
    ///
    /// Since: Unreleased.
    pub fn encode(&self, output: &mut [u8]) -> Result<usize, Code> {
        let length = match self {
            Self::Query | Self::Commit | Self::Abort => 1,
            Self::Begin { .. } => BEGIN_LEN,
            Self::Data { data, .. }
                if !data.is_empty() && data.len() <= MAX_FRAME - DATA_HEADER_LEN =>
            {
                DATA_HEADER_LEN + data.len()
            }
            Self::Data { .. } => return Err(Code::Size),
        };
        if output.len() < length {
            return Err(Code::Size);
        }
        match self {
            Self::Query => output[0] = OP_QUERY,
            Self::Begin { image, size, crc32 } => {
                output[0] = OP_BEGIN;
                output[1] = *image as u8;
                output[2..6].copy_from_slice(&size.to_le_bytes());
                output[6..BEGIN_LEN].copy_from_slice(&crc32.to_le_bytes());
            }
            Self::Data { offset, data } => {
                output[0] = OP_DATA;
                output[1..DATA_HEADER_LEN].copy_from_slice(&offset.to_le_bytes());
                output[DATA_HEADER_LEN..length].copy_from_slice(data);
            }
            Self::Commit => output[0] = OP_COMMIT,
            Self::Abort => output[0] = OP_ABORT,
        }
        Ok(length)
    }
}

/// Device state reported after every request.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Status {
    /// Result of the last request. Cleared by the next successful request.
    pub error: Code,
    /// Application slot an update would be written to: 0 for a device with a single update slot
    /// (marker board), 1 for bank A, 2 for bank B. The host picks the matching manifest image.
    pub app_slot: u8,
    /// Bitmask of [`Image::bit`] for the images the device can receive.
    pub supported: u8,
    /// Bitmask of [`Image::bit`] for images fully received and verified, awaiting `Commit`.
    pub staged: u8,
    /// Image of the transfer in progress.
    pub receiving: Option<Image>,
    /// Bytes of the transfer in progress already written. The next `Data` offset.
    pub offset: u32,
    /// Running application version as major, minor, patch.
    pub firmware_version: [u32; 3],
    /// Release number of the running network core firmware. 0 on devices that report none (the
    /// marker board).
    pub netcore_release: u16,
}

impl Status {
    /// Layout: version, error code, `app_slot`, a flags byte (`supported` in bits 0-1, `staged` in
    /// bits 2-3, `receiving` in bits 4-5 as 0 none, 1 application, 2 net core), `offset` as
    /// `u32`, three `u32` firmware version words, `netcore_release` as `u16`. Little endian.
    ///
    /// Since: Unreleased.
    pub fn encode(self) -> [u8; STATUS_LEN] {
        let receiving = match self.receiving {
            None => 0,
            Some(Image::Application) => 1,
            Some(Image::NetCore) => 2,
        };
        let mut output = [0; STATUS_LEN];
        output[0] = PROTOCOL_VERSION;
        output[1] = self.error as u8;
        output[2] = self.app_slot;
        output[3] = (self.supported & 3) | ((self.staged & 3) << 2) | (receiving << 4);
        output[4..8].copy_from_slice(&self.offset.to_le_bytes());
        for (index, version) in self.firmware_version.into_iter().enumerate() {
            let start = 8 + index * 4;
            output[start..start + 4].copy_from_slice(&version.to_le_bytes());
        }
        output[20..22].copy_from_slice(&self.netcore_release.to_le_bytes());
        output
    }

    /// Decodes a status report.
    ///
    /// # Errors
    ///
    /// Returns [`Code::InvalidFrame`] if the length, version, slot, flags or
    /// error code is invalid.
    ///
    /// Since: Unreleased.
    pub fn decode(input: &[u8]) -> Result<Self, Code> {
        if input.len() != STATUS_LEN || input[0] != PROTOCOL_VERSION {
            return Err(Code::InvalidFrame);
        }
        let error = code_from_byte(input[1]).ok_or(Code::InvalidFrame)?;
        let flags = input[3];
        let receiving = match (flags >> 4) & 3 {
            0 => None,
            1 => Some(Image::Application),
            2 => Some(Image::NetCore),
            _ => return Err(Code::InvalidFrame),
        };
        if input[2] > 2 || flags & 0xc0 != 0 {
            return Err(Code::InvalidFrame);
        }
        Ok(Self {
            error,
            app_slot: input[2],
            supported: flags & 3,
            staged: (flags >> 2) & 3,
            receiving,
            offset: le_u32(&input[4..8])?,
            firmware_version: [
                le_u32(&input[8..12])?,
                le_u32(&input[12..16])?,
                le_u32(&input[16..20])?,
            ],
            netcore_release: le_u16(&input[20..22])?,
        })
    }
}

fn le_u32(bytes: &[u8]) -> Result<u32, Code> {
    bytes
        .try_into()
        .map(u32::from_le_bytes)
        .or(Err(Code::InvalidFrame))
}

fn le_u16(bytes: &[u8]) -> Result<u16, Code> {
    bytes
        .try_into()
        .map(u16::from_le_bytes)
        .or(Err(Code::InvalidFrame))
}

fn code_from_byte(value: u8) -> Option<Code> {
    Some(match value {
        0 => Code::Ok,
        1 => Code::InvalidFrame,
        2 => Code::Busy,
        3 => Code::Unsupported,
        4 => Code::Size,
        5 => Code::Offset,
        6 => Code::Alignment,
        7 => Code::Flash,
        8 => Code::Checksum,
        9 => Code::Image,
        10 => Code::State,
        _ => return None,
    })
}

/// CRC-32/ISO-HDLC (the zlib and Ethernet CRC) of `bytes`.
///
/// Since: Unreleased.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(bytes);
    crc.finish()
}

/// Incremental [`crc32`], bitwise so it needs no lookup table in firmware.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug)]
pub struct Crc32(u32);

impl Crc32 {
    /// Since: Unreleased.
    pub const fn new() -> Self {
        Self(!0)
    }
    /// Since: Unreleased.
    pub fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u32::from(*byte);
            for _ in 0..8 {
                // Reflected form of the polynomial 0x04c1_1db7.
                self.0 = (self.0 >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(self.0 & 1)));
            }
        }
    }
    /// Since: Unreleased.
    pub const fn finish(self) -> u32 {
        !self.0
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{Code, Crc32, Image, Request, Status, crc32};

    #[test]
    fn crc32_matches_ieee_vector_and_streaming_updates() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        let mut crc = Crc32::new();
        crc.update(b"1234");
        crc.update(b"56789");
        assert_eq!(crc.finish(), crc32(b"123456789"));
    }

    #[test]
    fn request_codec_enforces_exact_lengths_and_frame_bounds() {
        let begin = Request::Begin {
            image: Image::NetCore,
            size: 0x1234_5678,
            crc32: 0x90ab_cdef,
        };
        let mut frame = [0; super::MAX_FRAME];
        assert_eq!(begin.encode(&mut frame), Ok(10));
        assert_eq!(
            &frame[..10],
            &[1, 1, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab, 0x90]
        );
        assert_eq!(Request::decode(&frame[..10]), Ok(begin));
        assert_eq!(Request::decode(&[0]), Ok(Request::Query));
        assert_eq!(Request::decode(&[0, 0]), Err(Code::InvalidFrame));
        assert_eq!(
            Request::decode(&[1, 2, 0, 0, 0, 0, 0, 0, 0, 0]),
            Err(Code::Image)
        );
        assert_eq!(Request::decode(&[2, 0, 0, 0, 0]), Err(Code::InvalidFrame));
        assert_eq!(
            Request::decode(&[2, 0, 0, 0, 0, 1]),
            Ok(Request::Data {
                offset: 0,
                data: &[1]
            })
        );

        let data = [0x5a; super::MAX_FRAME - 5];
        let max = Request::Data {
            offset: 4,
            data: &data,
        };
        assert_eq!(max.encode(&mut frame), Ok(super::MAX_FRAME));
        assert_eq!(Request::decode(&frame), Ok(max));
        assert_eq!(max.encode(&mut [0; super::MAX_FRAME - 1]), Err(Code::Size));
        assert_eq!(
            Request::Data {
                offset: 0,
                data: &[]
            }
            .encode(&mut frame),
            Err(Code::Size)
        );
        assert_eq!(Request::decode(&[2; super::MAX_FRAME + 1]), Err(Code::Size));
        assert_eq!(Request::decode(&[9]), Err(Code::InvalidFrame));
    }

    #[test]
    fn status_codec_preserves_little_endian_fields_and_rejects_invalid_values() {
        let status = Status {
            error: Code::Checksum,
            app_slot: 2,
            supported: 3,
            staged: 1,
            receiving: Some(Image::NetCore),
            offset: 0x1234_5678,
            firmware_version: [0x1234_5678, 0x89ab_cdef, 0x0001_0000],
            netcore_release: 0xabcd,
        };
        let bytes = status.encode();
        assert_eq!(
            bytes,
            [
                1, 8, 2, 0x27, 0x78, 0x56, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab,
                0x89, 0, 0, 1, 0, 0xcd, 0xab,
            ]
        );
        assert_eq!(Status::decode(&bytes), Ok(status));
        for (index, value) in [(1, 11), (2, 3), (3, 0x30), (3, 0x40)] {
            let mut invalid = bytes;
            invalid[index] = value;
            assert_eq!(Status::decode(&invalid), Err(Code::InvalidFrame));
        }
        assert_eq!(Status::decode(&bytes[..21]), Err(Code::InvalidFrame));
    }
}
