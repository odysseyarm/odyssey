//! Version of the application firmware a device reports.
//!
//! Firmware publishes its version as text in USB string descriptor 4. Release
//! builds are either `X.Y.Z` (Stable) or `X.Y.Z-nightly.YYYYMMDD.tHHMMSS.g<12 hex>`
//! (Nightly), the form the release workflow stamps. Parsing that text into
//! numbers makes versions comparable by value and fixed-size, so a
//! [`FirmwareVersion`] crosses the C ABI without a string buffer.

use std::fmt;
use std::str::FromStr;

/// Release channel of installed application firmware.
///
/// Variants are ordered by `SemVer` precedence: a Nightly precedes the Stable
/// release that has the same numeric core.
///
/// Since: Odyssey Desktop (unreleased).
#[repr(u8)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FirmwareChannel {
    /// The device reported no readable version. Every field of its
    /// [`FirmwareVersion`] is zero.
    ///
    /// Since: Odyssey Desktop (unreleased).
    #[default]
    Unknown = 0,
    /// A build stamped with `-nightly.<date>.t<time>.g<commit>`.
    ///
    /// Since: Odyssey Desktop (unreleased).
    Nightly = 1,
    /// A plain `X.Y.Z` release.
    ///
    /// Since: Odyssey Desktop (unreleased).
    Stable = 2,
}

/// A firmware version as the device reports it.
///
/// Fields order as `SemVer` precedence does, so the derived ordering compares
/// versions: numeric core first, then a Nightly before its Stable release, then
/// the build time.
///
/// Since: Odyssey Desktop (unreleased).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FirmwareVersion {
    /// `[major, minor, patch]`.
    ///
    /// Since: Odyssey Desktop (unreleased).
    pub core: [u32; 3],
    /// Since: Odyssey Desktop (unreleased).
    pub channel: FirmwareChannel,
    /// UTC build date as `YYYYMMDD`, 0 unless Nightly.
    ///
    /// Since: Odyssey Desktop (unreleased).
    pub date: u32,
    /// UTC build time as `HHMMSS`, 0 unless Nightly.
    ///
    /// Since: Odyssey Desktop (unreleased).
    pub time: u32,
    /// First 12 hex digits of the source commit as raw bytes, zero unless Nightly.
    ///
    /// Since: Odyssey Desktop (unreleased).
    pub commit: [u8; 6],
}

impl FirmwareVersion {
    /// `self` unless the device reported no version.
    ///
    /// Since: Odyssey Desktop (unreleased).
    #[must_use]
    pub fn known(self) -> Option<Self> {
        (self.channel != FirmwareChannel::Unknown).then_some(self)
    }
}

/// The text is neither a Stable nor a Nightly Odyssey firmware version.
///
/// Since: Odyssey Desktop (unreleased).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not an Odyssey firmware version")]
pub struct ParseFirmwareVersionError;

impl FromStr for FirmwareVersion {
    type Err = ParseFirmwareVersionError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (core, nightly) = match text.split_once('-') {
            Some((core, nightly)) => (core, Some(nightly)),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let (Some(major), Some(minor), Some(patch), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(ParseFirmwareVersionError);
        };
        let core = [number(major)?, number(minor)?, number(patch)?];
        match nightly {
            None => Ok(Self {
                core,
                channel: FirmwareChannel::Stable,
                ..Self::default()
            }),
            Some(nightly) => nightly_stamp(core, nightly),
        }
    }
}

impl fmt::Display for FirmwareVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch] = self.core;
        write!(f, "{major}.{minor}.{patch}")?;
        if self.channel == FirmwareChannel::Nightly {
            write!(f, "-nightly.{:08}.t{:06}.g", self.date, self.time)?;
            for byte in self.commit {
                write!(f, "{byte:02x}")?;
            }
        }
        Ok(())
    }
}

/// A `SemVer` numeric component: decimal digits without a leading zero.
fn number(text: &str) -> Result<u32, ParseFirmwareVersionError> {
    if text.len() > 1 && text.starts_with('0') {
        return Err(ParseFirmwareVersionError);
    }
    digits(text, 1..=10).ok_or(ParseFirmwareVersionError)
}

/// `text` as a number when it is ASCII digits and its length lies in `len`.
fn digits(text: &str, len: impl std::ops::RangeBounds<usize>) -> Option<u32> {
    if !len.contains(&text.len()) || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `nightly.YYYYMMDD.tHHMMSS.g<12 lowercase hex>`, the part after `X.Y.Z-`.
fn nightly_stamp(core: [u32; 3], text: &str) -> Result<FirmwareVersion, ParseFirmwareVersionError> {
    let mut parts = text.split('.');
    let (Some("nightly"), Some(date), Some(time), Some(commit), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err(ParseFirmwareVersionError);
    };
    let date = digits(date, 8..=8);
    let time = time.strip_prefix('t').and_then(|time| digits(time, 6..=6));
    let commit = commit.strip_prefix('g').and_then(commit_bytes);
    let (Some(date), Some(time), Some(commit)) = (date, time, commit) else {
        return Err(ParseFirmwareVersionError);
    };
    Ok(FirmwareVersion {
        core,
        channel: FirmwareChannel::Nightly,
        date,
        time,
        commit,
    })
}

/// 12 lowercase hex digits as 6 bytes. Uppercase is refused so [`fmt::Display`]
/// reproduces the input.
fn commit_bytes(hex: &str) -> Option<[u8; 6]> {
    if hex.len() != 12 {
        return None;
    }
    let mut commit = [0; 6];
    for (byte, [high, low]) in commit.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
        *byte = (nibble(*high)? << 4) | nibble(*low)?;
    }
    Some(commit)
}

fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{FirmwareChannel, FirmwareVersion};

    const NIGHTLY: &str = "1.2.4-nightly.20261111.t201500.gabcdef012345";

    fn version(text: &str) -> FirmwareVersion {
        text.parse()
            .unwrap_or_else(|error| panic!("{text:?} must parse: {error}"))
    }

    #[test]
    fn stable_and_nightly_text_round_trips() {
        for text in ["0.0.0", "1.2.3", "4294967295.0.1", NIGHTLY] {
            assert_eq!(version(text).to_string(), text);
        }
        let nightly = version(NIGHTLY);
        assert_eq!(nightly.core, [1, 2, 4]);
        assert_eq!(nightly.channel, FirmwareChannel::Nightly);
        assert_eq!(nightly.date, 20_261_111);
        assert_eq!(nightly.time, 201_500);
        assert_eq!(nightly.commit, [0xab, 0xcd, 0xef, 0x01, 0x23, 0x45]);
    }

    #[test]
    fn nightly_time_keeps_its_leading_zeros() {
        let text = "1.2.4-nightly.20261111.t020000.g000000000001";
        assert_eq!(version(text).time, 20_000);
        assert_eq!(version(text).to_string(), text);
    }

    #[test]
    fn unreported_version_is_not_known() {
        assert_eq!(FirmwareVersion::default().known(), None);
        assert_eq!(version("0.0.0").known(), Some(version("0.0.0")));
    }

    #[test]
    fn ordering_follows_semver_precedence() {
        let ascending = [
            "1.2.3",
            "1.2.4-nightly.20261111.t201500.gabcdef012345",
            "1.2.4-nightly.20261111.t201501.g000000000000",
            "1.2.4-nightly.20261112.t000000.g000000000000",
            "1.2.4",
            "1.10.0-nightly.20260101.t000000.g000000000000",
            "1.10.0",
            "2.0.0",
        ];
        for pair in ascending.windows(2) {
            assert!(
                version(pair[0]) < version(pair[1]),
                "{} must precede {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn other_text_is_refused() {
        for text in [
            "",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "01.2.3",
            "1.2.-3",
            "4294967296.0.0",
            "1.2.3+build",
            "1.2.3-nightly",
            "1.2.3-nightly.1",
            "1.2.3-rc.1",
            "1.2.3-nightly.2026111.t201500.gabcdef012345",
            "1.2.3-nightly.20261111.201500.gabcdef012345",
            "1.2.3-nightly.20261111.t20150.gabcdef012345",
            "1.2.3-nightly.20261111.t201500.gabcdef01234",
            "1.2.3-nightly.20261111.t201500.gABCDEF012345",
            "1.2.3-nightly.20261111.t201500.gabcdef012345.x",
        ] {
            assert!(text.parse::<FirmwareVersion>().is_err(), "{text:?}");
        }
    }
}
