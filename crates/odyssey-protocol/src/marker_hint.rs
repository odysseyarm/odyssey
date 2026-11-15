//! Hub-to-device marker hint, carried in a vendor packet.
//!
//! Old firmware ignores unknown vendor tags, so the hint needs no protocol bump.

/// Vendor tag of a marker hint. [`crate::ota::VENDOR_TAG`] is `0xfe`.
///
/// Since: Unreleased.
pub const VENDOR_TAG: u8 = 0xfd;
/// Since: Unreleased.
pub const MAX_POINTS: usize = 6;
/// Length byte, then `MAX_POINTS` little-endian `(x, y)` pairs, zero padded.
///
/// Since: Unreleased.
pub const ENCODED_LEN: usize = 1 + MAX_POINTS * 4;

/// Sensor-report positions (1/64 native px) of the spots the hub matched to the marker pattern.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarkerHint {
    /// Entries past `len` are zero.
    ///
    /// Since: Unreleased.
    pub points: [[u16; 2]; MAX_POINTS],
    /// Since: Unreleased.
    pub len: u8,
}

/// Why a byte string is not a canonical hint. Every variant rejects the packet; none is fatal to
/// the link.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkerHintError {
    /// The input is not exactly [`ENCODED_LEN`] bytes.
    ///
    /// Since: Unreleased.
    Length,
    /// The count byte exceeds [`MAX_POINTS`].
    ///
    /// Since: Unreleased.
    Count,
    /// A byte past the last point is not zero.
    ///
    /// Since: Unreleased.
    Padding,
}

impl core::fmt::Display for MarkerHintError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Length => "marker hint has the wrong length",
            Self::Count => "marker hint has too many points",
            Self::Padding => "marker hint padding is not zero",
        })
    }
}

impl core::error::Error for MarkerHintError {}

impl MarkerHint {
    /// Keeps the first [`MAX_POINTS`] points.
    ///
    /// Since: Unreleased.
    pub fn new(points: &[[u16; 2]]) -> Self {
        let len = points.len().min(MAX_POINTS);
        let mut hint = Self {
            points: [[0; 2]; MAX_POINTS],
            len: len as u8,
        };
        hint.points[..len].copy_from_slice(&points[..len]);
        hint
    }

    /// Since: Unreleased.
    pub fn points(&self) -> &[[u16; 2]] {
        &self.points[..usize::from(self.len).min(MAX_POINTS)]
    }

    /// Since: Unreleased.
    pub fn encode(&self) -> [u8; ENCODED_LEN] {
        let mut bytes = [0; ENCODED_LEN];
        let points = self.points();
        bytes[0] = points.len() as u8;
        for (i, point) in points.iter().enumerate() {
            let at = 1 + i * 4;
            bytes[at..at + 2].copy_from_slice(&point[0].to_le_bytes());
            bytes[at + 2..at + 4].copy_from_slice(&point[1].to_le_bytes());
        }
        bytes
    }

    /// # Errors
    /// Returns an error unless `bytes` is exactly what [`MarkerHint::encode`] produces.
    ///
    /// Since: Unreleased.
    pub fn decode(bytes: &[u8]) -> Result<Self, MarkerHintError> {
        let Ok(bytes) = <&[u8; ENCODED_LEN]>::try_from(bytes) else {
            return Err(MarkerHintError::Length);
        };
        let len = usize::from(bytes[0]);
        if len > MAX_POINTS {
            return Err(MarkerHintError::Count);
        }
        if bytes[1 + len * 4..].iter().any(|b| *b != 0) {
            return Err(MarkerHintError::Padding);
        }
        let mut points = [[0; 2]; MAX_POINTS];
        for (i, point) in points.iter_mut().enumerate().take(len) {
            let at = 1 + i * 4;
            *point = [
                u16::from_le_bytes([bytes[at], bytes[at + 1]]),
                u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]),
            ];
        }
        Ok(Self {
            points,
            len: bytes[0],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ENCODED_LEN, MAX_POINTS, MarkerHint, MarkerHintError};

    #[test]
    fn encode_is_little_endian_and_zero_padded() {
        let hint = MarkerHint::new(&[[0x0102, 0x0304], [0xfffe, 1]]);
        let bytes = hint.encode();
        assert_eq!(&bytes[..9], &[2, 2, 1, 4, 3, 0xfe, 0xff, 1, 0]);
        assert!(bytes[9..].iter().all(|b| *b == 0));
        assert_eq!(MarkerHint::decode(&bytes), Ok(hint));
    }

    #[test]
    fn roundtrips_every_length() {
        let all: [[u16; 2]; MAX_POINTS] =
            core::array::from_fn(|i| [i as u16 * 999 + 1, 20480 - i as u16]);
        for n in 0..=MAX_POINTS {
            let hint = MarkerHint::new(&all[..n]);
            assert_eq!(hint.points(), &all[..n], "{n}");
            assert_eq!(MarkerHint::decode(&hint.encode()), Ok(hint), "{n}");
        }
    }

    #[test]
    fn new_truncates_to_max_points() {
        let many = [[7, 8]; MAX_POINTS + 3];
        assert_eq!(MarkerHint::new(&many).points().len(), MAX_POINTS);
    }

    #[test]
    fn decode_rejects_wrong_length() {
        let bytes = MarkerHint::new(&[[1, 2]]).encode();
        assert_eq!(MarkerHint::decode(&[]), Err(MarkerHintError::Length));
        assert_eq!(
            MarkerHint::decode(&bytes[..ENCODED_LEN - 1]),
            Err(MarkerHintError::Length)
        );
        let mut long = [0; ENCODED_LEN + 1];
        long[..ENCODED_LEN].copy_from_slice(&bytes);
        assert_eq!(MarkerHint::decode(&long), Err(MarkerHintError::Length));
    }

    #[test]
    fn decode_rejects_count_over_max() {
        let mut bytes = [0; ENCODED_LEN];
        bytes[0] = MAX_POINTS as u8 + 1;
        assert_eq!(MarkerHint::decode(&bytes), Err(MarkerHintError::Count));
        bytes[0] = u8::MAX;
        assert_eq!(MarkerHint::decode(&bytes), Err(MarkerHintError::Count));
    }

    #[test]
    fn decode_rejects_nonzero_padding() {
        for count in 0..MAX_POINTS {
            let mut bytes = MarkerHint::new(&[[5, 6]; MAX_POINTS][..count]).encode();
            bytes[ENCODED_LEN - 1] = 1;
            assert_eq!(
                MarkerHint::decode(&bytes),
                Err(MarkerHintError::Padding),
                "{count}"
            );
        }
    }
}
