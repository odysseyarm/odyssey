//! Hub-to-device marker hint, carried in [`crate::PacketData::MarkerHint`].
//!
//! The hub builds a hint from the spots it matched to the marker pattern
//! (`crates/odyssey-hub-server/src/device_tasks/marker_hint_sender.rs`) and sends it as a one-way
//! packet. The tracker firmware checks it with [`MarkerHint::validate`] in
//! `firmware/atslite-common/src/object_mode.rs` and uses it to restrict its exposure controller to
//! real markers, so a malformed hint is dropped and never half-applied.

use odyssey_protocol_macros::protocol_type;

/// Since: Unreleased.
pub const MAX_POINTS: usize = 6;

/// Sensor-report positions (1/64 native px) of the spots the hub matched to the marker pattern.
///
/// Since: Unreleased.
#[protocol_type]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarkerHint {
    /// Entries past `len` are zero.
    ///
    /// Since: Unreleased.
    #[cfg_attr(feature = "minicbor", n(0))]
    pub points: [[u16; 2]; MAX_POINTS],
    /// Since: Unreleased.
    #[cfg_attr(feature = "minicbor", n(1))]
    pub len: u8,
}

/// Why a hint is not one [`MarkerHint::new`] could have built. Every variant rejects the packet;
/// none is fatal to the link.
///
/// Since: Unreleased.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkerHintError {
    /// `len` exceeds [`MAX_POINTS`].
    ///
    /// Since: Unreleased.
    Count,
    /// A point past `len` is not zero.
    ///
    /// Since: Unreleased.
    Padding,
}

impl core::fmt::Display for MarkerHintError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
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

    /// # Errors
    /// Returns an error unless the hint is one [`MarkerHint::new`] could have built.
    ///
    /// Since: Unreleased.
    pub fn validate(&self) -> Result<(), MarkerHintError> {
        let len = usize::from(self.len);
        if len > MAX_POINTS {
            return Err(MarkerHintError::Count);
        }
        if self.points[len..].iter().any(|point| *point != [0, 0]) {
            return Err(MarkerHintError::Padding);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_POINTS, MarkerHint, MarkerHintError};

    #[test]
    fn new_keeps_points_and_validates_every_length() {
        let all: [[u16; 2]; MAX_POINTS] =
            core::array::from_fn(|i| [i as u16 * 999 + 1, 20480 - i as u16]);
        for n in 0..=MAX_POINTS {
            let hint = MarkerHint::new(&all[..n]);
            assert_eq!(hint.points(), &all[..n], "{n}");
            assert_eq!(hint.validate(), Ok(()), "{n}");
        }
    }

    #[test]
    fn new_truncates_to_max_points() {
        let many = [[7, 8]; MAX_POINTS + 3];
        assert_eq!(MarkerHint::new(&many).points().len(), MAX_POINTS);
    }

    #[test]
    fn validate_rejects_count_over_max() {
        let mut hint = MarkerHint::new(&[]);
        hint.len = MAX_POINTS as u8 + 1;
        assert_eq!(hint.validate(), Err(MarkerHintError::Count));
        hint.len = u8::MAX;
        assert_eq!(hint.validate(), Err(MarkerHintError::Count));
    }

    #[test]
    fn validate_rejects_nonzero_padding() {
        for count in 0..MAX_POINTS {
            let mut hint = MarkerHint::new(&[[5, 6]; MAX_POINTS][..count]);
            hint.points[MAX_POINTS - 1] = [0, 1];
            assert_eq!(hint.validate(), Err(MarkerHintError::Padding), "{count}");
        }
    }
}
