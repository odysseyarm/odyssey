//! The version record a stage2 bootloader leaves in its own flash region so the
//! application can report which bootloader is installed.
//!
//! Address rule: the record occupies the last [`LEN`] bytes of the bootloader's
//! flash region, so it sits at `region_end - LEN` ([`address`]). The bootloader
//! build script links the static there; apps and the host tooling find it from the
//! region end in `memory-*.kdl`, which every image already knows. An old
//! bootloader has no record: its last bytes are erased flash or code, and
//! [`BootloaderInfo::version_from_bytes`] answers `[0, 0, 0]` for them.

#![no_std]

/// Marks a record: the bytes of `"OBLV"` read as a big-endian number, stored
/// little-endian like the rest of the record.
///
/// Since: Unreleased.
pub const MAGIC: u32 = 0x4F42_4C56;

/// Size of the record in bytes: the magic and three version words.
///
/// Since: Unreleased.
pub const LEN: usize = 16;

/// The record a bootloader embeds. Layout is frozen: magic then the three
/// bootloader version words, all little-endian.
///
/// Since: Unreleased.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootloaderInfo {
    magic: u32,
    version: [u32; 3],
}

const _: () = assert!(size_of::<BootloaderInfo>() == LEN);

impl BootloaderInfo {
    /// The record for a bootloader at `version`.
    ///
    /// Since: Unreleased.
    #[must_use]
    pub const fn new(version: [u32; 3]) -> Self {
        Self {
            magic: MAGIC,
            version,
        }
    }

    /// The version the record carries when `bytes` holds a record, `[0, 0, 0]`
    /// otherwise (a bootloader that predates the record).
    ///
    /// Since: Unreleased.
    #[must_use]
    pub fn version_from_bytes(bytes: &[u8; LEN]) -> [u32; 3] {
        let word = |index: usize| {
            let start = index * 4;
            u32::from_le_bytes([
                bytes[start],
                bytes[start + 1],
                bytes[start + 2],
                bytes[start + 3],
            ])
        };
        if word(0) != MAGIC {
            return [0, 0, 0];
        }
        [word(1), word(2), word(3)]
    }

    /// The record as stored in flash.
    ///
    /// Since: Unreleased.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; LEN] {
        let mut bytes = [0u8; LEN];
        let words = [
            self.magic,
            self.version[0],
            self.version[1],
            self.version[2],
        ];
        let mut index = 0;
        while index < 4 {
            let word = words[index].to_le_bytes();
            let mut byte = 0;
            while byte < 4 {
                bytes[index * 4 + byte] = word[byte];
                byte += 1;
            }
            index += 1;
        }
        bytes
    }
}

/// Flash address of the record for a bootloader region that ends at `region_end`
/// (exclusive).
///
/// Since: Unreleased.
#[must_use]
pub const fn address(region_end: u32) -> u32 {
    region_end - LEN as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_layout_is_frozen() {
        let info = BootloaderInfo::new([1, 2, 3]);
        assert_eq!(
            info.to_bytes(),
            [0x56, 0x4C, 0x42, 0x4F, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0]
        );
        assert_eq!(
            BootloaderInfo::version_from_bytes(&info.to_bytes()),
            [1, 2, 3]
        );
    }

    #[test]
    fn missing_magic_reads_as_zero() {
        assert_eq!(BootloaderInfo::version_from_bytes(&[0xFF; LEN]), [0, 0, 0]);
        assert_eq!(BootloaderInfo::version_from_bytes(&[0; LEN]), [0, 0, 0]);
    }

    #[test]
    fn record_sits_in_last_bytes_of_region() {
        assert_eq!(address(0x2000 + 44 * 1024), 0xCFF0);
    }
}
