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
//! | 2      | [`Request::Data`]   | offset `u32`, then image bytes         | 6 to 245     |
//! | 3      | [`Request::Commit`] | none                                   | 1            |
//! | 4      | [`Request::Abort`]  | none                                   | 1            |
//! | 5      | [`Request::Signature`] | image `u8`, record offset `u8`, record bytes (version `[u32; 3]`, signature `[u8; 64]`; 76 in all) | 4 to 79 |
//!
//! The status reply is [`STATUS_LEN`] bytes. See [`Status::encode`] for the layout.

/// Largest request frame: one opcode byte, a 4-byte offset and up to 240 data bytes.
///
/// The unit's BLE host reassembles each received L2CAP SDU inside one 251-byte pool packet and
/// closes the channel on anything larger, whatever MTU the channel announces. A
/// [`crate::PacketData::OtaRequest`] carrying this frame encodes to at most 249 bytes.
///
/// Since: Unreleased.
pub const MAX_FRAME: usize = 245;
/// One OTA request frame as a packet carries it.
///
/// Since: Unreleased.
pub type Frame = crate::List<u8, MAX_FRAME>;
/// Length of an encoded [`Status`].
///
/// Since: Unreleased.
pub const STATUS_LEN: usize = 22;
/// First byte of every status this build writes. Version 2 added [`Request::Signature`] and
/// [`Status::signature_required`].
///
/// Since: Unreleased.
pub const PROTOCOL_VERSION: u8 = 2;
/// Oldest status version [`Status::decode`] still reads, so a hub can update a device that
/// predates signatures. Such a device never requires one.
///
/// Since: Unreleased.
pub const MIN_PROTOCOL_VERSION: u8 = 1;
/// Length of the Ed25519 signature in a signature record.
///
/// Since: Unreleased.
pub const SIGNATURE_LEN: usize = 64;
/// Length of the signature record [`Request::Signature`] carries: three little-endian `u32`
/// version words, then the [`SIGNATURE_LEN`]-byte signature.
///
/// Since: Unreleased.
pub const SIGNATURE_RECORD_LEN: usize = 12 + SIGNATURE_LEN;
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
    /// The staged image has no signature or the signature does not verify against a trusted key
    /// (see `firmware/release-signing`). The receiver drops its staged state.
    Signature = 11,
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
const OP_SIGNATURE: u8 = 5;
/// Opcode, image, size, CRC-32.
const BEGIN_LEN: usize = 10;
/// Opcode and offset.
const DATA_HEADER_LEN: usize = 5;
/// Opcode, image, record offset.
const SIGNATURE_HEADER_LEN: usize = 3;

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
    /// Next chunk of the signature record for an image that is receiving or staged. `offset`
    /// must equal the record bytes received so far. The record is [`SIGNATURE_RECORD_LEN`]
    /// bytes: the version the signature binds the image to (application semver, or
    /// `[release, 0, 0]` for the net core) as three `u32`, then the Ed25519 signature.
    /// The host sizes chunks to the link, so a record fits a 20-byte GATT write in several
    /// frames. Checked at `Commit` by devices that require signatures and ignored by the others.
    Signature {
        image: Image,
        offset: u8,
        bytes: &'a [u8],
    },
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
            Some(&OP_BEGIN) if frame.len() == BEGIN_LEN => Ok(Self::Begin {
                image: image_from_byte(frame[1])?,
                size: le_u32(&frame[2..6])?,
                crc32: le_u32(&frame[6..10])?,
            }),
            Some(&OP_SIGNATURE) if frame.len() > SIGNATURE_HEADER_LEN => {
                let image = image_from_byte(frame[1])?;
                let offset = frame[2];
                let bytes = &frame[SIGNATURE_HEADER_LEN..];
                if usize::from(offset) + bytes.len() > SIGNATURE_RECORD_LEN {
                    return Err(Code::Size);
                }
                Ok(Self::Signature {
                    image,
                    offset,
                    bytes,
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
            Self::Signature { offset, bytes, .. }
                if !bytes.is_empty()
                    && usize::from(*offset) + bytes.len() <= SIGNATURE_RECORD_LEN =>
            {
                SIGNATURE_HEADER_LEN + bytes.len()
            }
            Self::Data { data, .. }
                if !data.is_empty() && data.len() <= MAX_FRAME - DATA_HEADER_LEN =>
            {
                DATA_HEADER_LEN + data.len()
            }
            Self::Data { .. } | Self::Signature { .. } => return Err(Code::Size),
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
            Self::Signature {
                image,
                offset,
                bytes,
            } => {
                output[0] = OP_SIGNATURE;
                output[1] = *image as u8;
                output[2] = *offset;
                output[3..length].copy_from_slice(bytes);
            }
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
    /// The device refuses to commit an image without a valid [`Request::Signature`]. Hosts send
    /// the signature whenever the manifest has one and fail early when this is set and it does
    /// not.
    pub signature_required: bool,
}

impl Status {
    /// Layout: version, error code, `app_slot`, a flags byte (`supported` in bits 0-1, `staged` in
    /// bits 2-3, `receiving` in bits 4-5 as 0 none, 1 application, 2 net core,
    /// `signature_required` in bit 6, bit 7 reserved zero), `offset` as `u32`, three `u32`
    /// firmware version words, `netcore_release` as `u16`. Little endian.
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
        output[3] = (self.supported & 3)
            | ((self.staged & 3) << 2)
            | (receiving << 4)
            | (u8::from(self.signature_required) << 6);
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
        if input.len() != STATUS_LEN
            || !(MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&input[0])
        {
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
        if input[2] > 2
            || flags & 0x80 != 0
            || (input[0] == MIN_PROTOCOL_VERSION && (flags & 0x40 != 0 || error == Code::Signature))
        {
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
            signature_required: flags & 0x40 != 0,
        })
    }
}

fn image_from_byte(value: u8) -> Result<Image, Code> {
    match value {
        0 => Ok(Image::Application),
        1 => Ok(Image::NetCore),
        _ => Err(Code::Image),
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
        11 => Code::Signature,
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
    fn signature_chunks_have_frozen_layout_and_bounds() {
        let record: [u8; super::SIGNATURE_RECORD_LEN] = core::array::from_fn(|index| index as u8);
        let mut frame = [0; super::MAX_FRAME];
        let first = Request::Signature {
            image: Image::NetCore,
            offset: 0,
            bytes: &record[..12],
        };
        assert_eq!(first.encode(&mut frame), Ok(15));
        assert_eq!(
            &frame[..15],
            &[5, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]
        );
        assert_eq!(Request::decode(&frame[..15]), Ok(first));

        let last = Request::Signature {
            image: Image::Application,
            offset: 72,
            bytes: &record[72..],
        };
        assert_eq!(last.encode(&mut frame), Ok(7));
        assert_eq!(&frame[..7], &[5, 0, 72, 72, 73, 74, 75]);
        assert_eq!(Request::decode(&frame[..7]), Ok(last));

        let whole = Request::Signature {
            image: Image::Application,
            offset: 0,
            bytes: &record,
        };
        assert_eq!(whole.encode(&mut frame), Ok(79));
        assert_eq!(Request::decode(&frame[..79]), Ok(whole));

        assert_eq!(Request::decode(&[5, 0, 0]), Err(Code::InvalidFrame));
        assert_eq!(Request::decode(&[5, 2, 0, 1]), Err(Code::Image));
        assert_eq!(Request::decode(&[5, 0, 76, 1]), Err(Code::Size));
        assert_eq!(Request::decode(&[5, 0, 75, 1, 2]), Err(Code::Size));
        let oversize = Request::Signature {
            image: Image::Application,
            offset: 1,
            bytes: &record,
        };
        assert_eq!(oversize.encode(&mut frame), Err(Code::Size));
        let empty = Request::Signature {
            image: Image::Application,
            offset: 0,
            bytes: &[],
        };
        assert_eq!(empty.encode(&mut frame), Err(Code::Size));
        assert_eq!(first.encode(&mut [0; 14]), Err(Code::Size));
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
            signature_required: true,
        };
        let bytes = status.encode();
        assert_eq!(
            bytes,
            [
                2, 8, 2, 0x67, 0x78, 0x56, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab,
                0x89, 0, 0, 1, 0, 0xcd, 0xab,
            ]
        );
        assert_eq!(Status::decode(&bytes), Ok(status));
        for (index, value) in [(0, 0), (0, 3), (1, 12), (2, 3), (3, 0x30 | 0x40), (3, 0xe7)] {
            let mut invalid = bytes;
            invalid[index] = value;
            assert_eq!(
                Status::decode(&invalid),
                Err(Code::InvalidFrame),
                "{index} {value}"
            );
        }
        let mut legacy = bytes;
        legacy[0] = 1;
        assert_eq!(Status::decode(&legacy), Err(Code::InvalidFrame));
        legacy[3] &= !0x40;
        legacy[1] = Code::Signature as u8;
        assert_eq!(Status::decode(&legacy), Err(Code::InvalidFrame));
        legacy[1] = Code::Checksum as u8;
        let legacy = Status::decode(&legacy).expect("version 1 status decodes");
        assert!(!legacy.signature_required);
        assert_eq!(Status::decode(&bytes[..21]), Err(Code::InvalidFrame));
    }
}
