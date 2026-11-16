//! The installed stage2 bootloader's version, read from the record it leaves at the end of
//! its flash region (see the `bootloader-info` crate).

use bootloader_info::{BootloaderInfo, LEN};

/// Version of the stage2 bootloader, `[0, 0, 0]` when it predates the record.
///
/// The region end comes from `__stage2_bootloader_end` in `memory-*.kdl`, the same symbol the
/// bootloader's own build script derives the record address from.
///
/// Since: Unreleased.
pub fn bootloader_version() -> [u32; 3] {
    unsafe extern "C" {
        static __stage2_bootloader_end: u32;
    }
    let region_end = (&raw const __stage2_bootloader_end) as u32;
    let address = bootloader_info::address(region_end) as *const [u8; LEN];
    // SAFETY: The record address is the last LEN bytes of the stage2 bootloader flash region,
    // which is always mapped and readable; the bytes are plain data and any content parses.
    let bytes = unsafe { core::ptr::read_volatile(address) };
    BootloaderInfo::version_from_bytes(&bytes)
}
