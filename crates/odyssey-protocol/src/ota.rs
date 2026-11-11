//! Raw OTA request and status framing.

/// Since: Unreleased.
pub const VENDOR_TAG: u8 = 0xfe;
/// Since: Unreleased.
pub const MAX_FRAME: usize = 98;
/// Since: Unreleased.
pub const STATUS_LEN: usize = 22;
/// Since: Unreleased.
pub const PROTOCOL_VERSION: u8 = 1;
/// Since: Unreleased.
pub const MARKER_OTA_MUX_PROTOCOL: [u16; 3] = [0, 6, 0];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
/// Since: Unreleased.
pub enum Image {
    Application = 0,
    NetCore = 1,
}

impl Image {
    /// Since: Unreleased.
    pub const fn bit(self) -> u8 {
        1 << self as u8
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
/// Since: Unreleased.
pub enum Code {
    Ok = 0,
    InvalidFrame = 1,
    Busy = 2,
    Unsupported = 3,
    Size = 4,
    Offset = 5,
    Alignment = 6,
    Flash = 7,
    Checksum = 8,
    Image = 9,
    State = 10,
}

impl core::fmt::Display for Code {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "OTA error {}", *self as u8)
    }
}

impl core::error::Error for Code {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Since: Unreleased.
pub enum Request<'a> {
    Query,
    Begin { image: Image, size: u32, crc32: u32 },
    Data { offset: u32, data: &'a [u8] },
    Commit,
    Abort,
}

impl<'a> Request<'a> {
    /// Since: Unreleased.
    pub fn decode(frame: &'a [u8]) -> Result<Self, Code> {
        match frame.first() {
            Some(0) if frame.len() == 1 => Ok(Self::Query),
            Some(1) if frame.len() == 10 => {
                let image = match frame[1] {
                    0 => Image::Application,
                    1 => Image::NetCore,
                    _ => return Err(Code::Image),
                };
                Ok(Self::Begin {
                    image,
                    size: u32::from_le_bytes(frame[2..6].try_into().map_err(|_| Code::InvalidFrame)?),
                    crc32: u32::from_le_bytes(frame[6..10].try_into().map_err(|_| Code::InvalidFrame)?),
                })
            }
            Some(2) if (6..=MAX_FRAME).contains(&frame.len()) => Ok(Self::Data {
                offset: u32::from_le_bytes(frame[1..5].try_into().map_err(|_| Code::InvalidFrame)?),
                data: &frame[5..],
            }),
            Some(3) if frame.len() == 1 => Ok(Self::Commit),
            Some(4) if frame.len() == 1 => Ok(Self::Abort),
            Some(1) => Err(Code::InvalidFrame),
            Some(2) if frame.len() < 6 => Err(Code::InvalidFrame),
            Some(2) => Err(Code::Size),
            Some(_) => Err(Code::InvalidFrame),
            None => Err(Code::InvalidFrame),
        }
    }

    /// Since: Unreleased.
    pub fn encode(&self, output: &mut [u8]) -> Result<usize, Code> {
        let length = match self {
            Self::Query | Self::Commit | Self::Abort => 1,
            Self::Begin { .. } => 10,
            Self::Data { data, .. } if !data.is_empty() && data.len() <= MAX_FRAME - 5 => 5 + data.len(),
            Self::Data { .. } => return Err(Code::Size),
        };
        if output.len() < length { return Err(Code::Size); }
        match self {
            Self::Query => output[0] = 0,
            Self::Begin { image, size, crc32 } => {
                output[0] = 1;
                output[1] = *image as u8;
                output[2..6].copy_from_slice(&size.to_le_bytes());
                output[6..10].copy_from_slice(&crc32.to_le_bytes());
            }
            Self::Data { offset, data } => {
                output[0] = 2;
                output[1..5].copy_from_slice(&offset.to_le_bytes());
                output[5..length].copy_from_slice(data);
            }
            Self::Commit => output[0] = 3,
            Self::Abort => output[0] = 4,
        }
        Ok(length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Since: Unreleased.
pub struct Status {
    pub error: Code,
    pub app_slot: u8,
    pub supported: u8,
    pub staged: u8,
    pub receiving: Option<Image>,
    pub offset: u32,
    pub firmware_version: [u32; 3],
    pub netcore_release: u16,
}

impl Status {
    /// Since: Unreleased.
    pub fn encode(self) -> [u8; STATUS_LEN] {
        let receiving = match self.receiving { None => 0, Some(Image::Application) => 1, Some(Image::NetCore) => 2 };
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

    /// Since: Unreleased.
    pub fn decode(input: &[u8]) -> Result<Self, Code> {
        if input.len() != STATUS_LEN || input[0] != PROTOCOL_VERSION { return Err(Code::InvalidFrame); }
        let error = code_from_byte(input[1]).ok_or(Code::InvalidFrame)?;
        let flags = input[3];
        let receiving = match (flags >> 4) & 3 {
            0 => None,
            1 => Some(Image::Application),
            2 => Some(Image::NetCore),
            _ => return Err(Code::InvalidFrame),
        };
        if input[2] > 2 || flags & 0xc0 != 0 { return Err(Code::InvalidFrame); }
        Ok(Self {
            error,
            app_slot: input[2],
            supported: flags & 3,
            staged: (flags >> 2) & 3,
            receiving,
            offset: u32::from_le_bytes(input[4..8].try_into().map_err(|_| Code::InvalidFrame)?),
            firmware_version: [
                u32::from_le_bytes(input[8..12].try_into().map_err(|_| Code::InvalidFrame)?),
                u32::from_le_bytes(input[12..16].try_into().map_err(|_| Code::InvalidFrame)?),
                u32::from_le_bytes(input[16..20].try_into().map_err(|_| Code::InvalidFrame)?),
            ],
            netcore_release: u16::from_le_bytes(input[20..22].try_into().map_err(|_| Code::InvalidFrame)?),
        })
    }
}

fn code_from_byte(value: u8) -> Option<Code> {
    Some(match value {
        0 => Code::Ok, 1 => Code::InvalidFrame, 2 => Code::Busy, 3 => Code::Unsupported,
        4 => Code::Size, 5 => Code::Offset, 6 => Code::Alignment, 7 => Code::Flash,
        8 => Code::Checksum, 9 => Code::Image, 10 => Code::State, _ => return None,
    })
}

/// Since: Unreleased.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(bytes);
    crc.finish()
}

#[derive(Clone, Copy, Debug)]
/// Since: Unreleased.
pub struct Crc32(u32);

impl Crc32 {
    /// Since: Unreleased.
    pub const fn new() -> Self { Self(!0) }
    /// Since: Unreleased.
    pub fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u32::from(*byte);
            for _ in 0..8 { self.0 = (self.0 >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(self.0 & 1))); }
        }
    }
    /// Since: Unreleased.
    pub const fn finish(self) -> u32 { !self.0 }
}

impl Default for Crc32 { fn default() -> Self { Self::new() } }
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
        let begin = Request::Begin { image: Image::NetCore, size: 0x1234_5678, crc32: 0x90ab_cdef };
        let mut frame = [0; super::MAX_FRAME];
        assert_eq!(begin.encode(&mut frame), Ok(10));
        assert_eq!(&frame[..10], &[1, 1, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab, 0x90]);
        assert_eq!(Request::decode(&frame[..10]), Ok(begin));
        assert_eq!(Request::decode(&[0]), Ok(Request::Query));
        assert_eq!(Request::decode(&[0, 0]), Err(Code::InvalidFrame));
        assert_eq!(Request::decode(&[1, 2, 0, 0, 0, 0, 0, 0, 0, 0]), Err(Code::Image));
        assert_eq!(Request::decode(&[2, 0, 0, 0, 0]), Err(Code::InvalidFrame));
        assert_eq!(Request::decode(&[2, 0, 0, 0, 0, 1]), Ok(Request::Data { offset: 0, data: &[1] }));

        let data = [0x5a; super::MAX_FRAME - 5];
        let max = Request::Data { offset: 4, data: &data };
        assert_eq!(max.encode(&mut frame), Ok(super::MAX_FRAME));
        assert_eq!(Request::decode(&frame), Ok(max));
        assert_eq!(max.encode(&mut [0; super::MAX_FRAME - 1]), Err(Code::Size));
        assert_eq!(Request::Data { offset: 0, data: &[] }.encode(&mut frame), Err(Code::Size));
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
        assert_eq!(bytes, [
            1, 8, 2, 0x27, 0x78, 0x56, 0x34, 0x12,
            0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab, 0x89,
            0, 0, 1, 0, 0xcd, 0xab,
        ]);
        assert_eq!(Status::decode(&bytes), Ok(status));
        for (index, value) in [(1, 11), (2, 3), (3, 0x30), (3, 0x40)] {
            let mut invalid = bytes;
            invalid[index] = value;
            assert_eq!(Status::decode(&invalid), Err(Code::InvalidFrame));
        }
        assert_eq!(Status::decode(&bytes[..21]), Err(Code::InvalidFrame));
    }
}
