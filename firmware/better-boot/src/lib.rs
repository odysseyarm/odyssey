//! Dual-bank boot state kept at a flash address chosen at runtime, instead of
//! the fixed linker symbols embassy-boot requires.

#![no_std]
#![expect(
    unsafe_code,
    reason = "Boot state lives in memory-mapped flash, which requires volatile pointer access."
)]

use core::ptr;

use embedded_storage::nor_flash::NorFlash;

/// Value of [`BootState::magic`] once the state has been written. Specific to
/// this crate, since embassy-boot has no equivalent.
///
/// Since: Odyssey Desktop 0.6.0.
pub const STATE_MAGIC: u32 = 0xB00710AD;
/// Partition magic word for a normal boot. Same value as embassy-boot.
///
/// Since: Odyssey Desktop 0.6.0.
pub const BOOT_MAGIC: u32 = 0xD0D0D0D0;
/// Partition magic word that requests DFU mode. Same value as embassy-boot's
/// swap magic.
///
/// Since: Odyssey Desktop 0.6.0.
pub const SWAP_MAGIC: u32 = 0xF0F0F0F0;

/// Boot state stored in flash right after the 4-byte partition magic word.
///
/// Since: Odyssey Desktop 0.6.0.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BootState {
    /// [`STATE_MAGIC`] if the state was written by this crate. Anything else
    /// means erased or corrupt flash.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub magic: u32,
    /// Currently active slot (0 = Bank A, 1 = Bank B)
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub active_slot: u8,
    /// Pending slot to boot next (if different from active)
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub pending_slot: u8,
    /// Incremented by the bootloader each time it boots the pending slot.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub boot_counter: u8,
    /// Unused, always written as 0.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub reserved: u8,
}

impl BootState {
    /// Both slots on Bank A, boot counter at zero.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn new() -> Self {
        Self {
            magic: STATE_MAGIC,
            active_slot: 0,
            pending_slot: 0,
            boot_counter: 0,
            reserved: 0,
        }
    }

    /// True if the magic matches [`STATE_MAGIC`] and both bank slots are 0 or 1.
    /// A matching magic alone does not make corrupted slot bytes safe to boot.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn is_valid(&self) -> bool {
        self.magic == STATE_MAGIC && self.active_slot <= 1 && self.pending_slot <= 1
    }
}

impl Default for BootState {
    fn default() -> Self {
        Self::new()
    }
}
/// Validates initial stack pointer and Thumb reset vector against one app bank.
pub fn valid_vectors(bank: core::ops::Range<usize>, sp: u32, reset: u32) -> bool {
    (0x2000_0000..=0x2008_0000).contains(&sp)
        && sp.is_multiple_of(8)
        && reset & 1 == 1
        && bank.contains(&((reset & !1) as usize))
}

/// Applies pending-slot rollback and picks a valid bank, preferring active slot.
/// Returns `None` when neither bank is safe to boot.
pub fn select_boot_slot(state: &mut BootState, valid: [bool; 2]) -> Option<u8> {
    const MAX_BOOT_ATTEMPTS: u8 = 2;
    let active = state.active_slot;

    if state.pending_slot != active {
        let pending_valid = valid.get(state.pending_slot as usize) == Some(&true);
        if state.boot_counter >= MAX_BOOT_ATTEMPTS || !pending_valid {
            state.pending_slot = active;
            state.boot_counter = 0;
        } else {
            state.boot_counter = state.boot_counter.saturating_add(1);
            return Some(state.pending_slot);
        }
    }

    if valid.get(active as usize) == Some(&true) {
        Some(active)
    } else if valid[0] {
        Some(0)
    } else if valid[1] {
        Some(1)
    } else {
        None
    }
}

/// Reads and writes the boot state partition at `state_addr`: a 4-byte magic
/// word ([`BOOT_MAGIC`] or [`SWAP_MAGIC`]) followed by a [`BootState`].
///
/// Since: Odyssey Desktop 0.6.0.
pub struct BootStateManager {
    state_addr: u32,
}

impl BootStateManager {
    /// `state_addr` must be aligned and identify the start of readable,
    /// memory-mapped flash containing the magic word and [`BootState`].
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub const fn new(state_addr: u32) -> Self {
        Self { state_addr }
    }

    /// Reads the state stored after the magic word, or [`BootState::new`] if
    /// its magic or bank slots are invalid.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn read(&self) -> BootState {
        let state_ptr =
            (self.state_addr as usize + core::mem::size_of::<u32>()) as *const BootState;
        // SAFETY: Constructor contract guarantees memory-mapped flash. Adding
        // the 4-byte magic offset preserves BootState alignment. Integer fields
        // accept every bit pattern read from flash.
        let state = unsafe { ptr::read_volatile(state_ptr) };

        if state.is_valid() {
            state
        } else {
            BootState::new()
        }
    }

    /// Reads the magic word at the start of the state partition.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn read_magic(&self) -> u32 {
        // SAFETY: Constructor guarantees an aligned, readable flash address.
        unsafe { ptr::read_volatile(self.state_addr as *const u32) }
    }

    /// Erases the flash page holding the state, then writes [`BOOT_MAGIC`] and
    /// `state`. `page_size` must be a power of two.
    ///
    /// # Errors
    ///
    /// Returns the flash driver's error if the erase or a write fails.
    ///
    /// # Since
    /// Odyssey Desktop 0.6.0.
    pub fn write_boot_state<F: NorFlash>(
        &self,
        flash: &mut F,
        state: &BootState,
        page_size: u32,
    ) -> Result<(), F::Error> {
        let base = self.state_addr;
        let page_start = base & !(page_size - 1);
        let page_end = page_start + page_size;

        flash.erase(page_start, page_end)?;
        flash.write(base, &BOOT_MAGIC.to_le_bytes())?;

        // SAFETY: `state` is a live reference and `BootState` has repr(C),
        // alignment 4, and exactly eight initialized bytes with no padding.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                state as *const BootState as *const u8,
                core::mem::size_of::<BootState>(),
            )
        };
        let dst = base + core::mem::size_of::<u32>() as u32;
        flash.write(dst, bytes)?;

        Ok(())
    }

    /// Like [`Self::write_boot_state`], but writes [`SWAP_MAGIC`] so the next
    /// boot enters DFU mode.
    ///
    /// # Errors
    ///
    /// Returns the flash driver's error if the erase or a write fails.
    ///
    /// # Since
    /// Odyssey Desktop 0.6.0.
    pub fn request_dfu<F: NorFlash>(
        &self,
        flash: &mut F,
        state: &BootState,
        page_size: u32,
    ) -> Result<(), F::Error> {
        let base = self.state_addr;
        let page_start = base & !(page_size - 1);
        let page_end = page_start + page_size;

        flash.erase(page_start, page_end)?;
        flash.write(base, &SWAP_MAGIC.to_le_bytes())?;

        // SAFETY: `state` is a live reference and `BootState` has repr(C),
        // alignment 4, and exactly eight initialized bytes with no padding.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                state as *const BootState as *const u8,
                core::mem::size_of::<BootState>(),
            )
        };
        let dst = base + core::mem::size_of::<u32>() as u32;
        flash.write(dst, bytes)?;

        Ok(())
    }

    /// True if the partition magic word is [`SWAP_MAGIC`].
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn is_dfu_requested(&self) -> bool {
        self.read_magic() == SWAP_MAGIC
    }

    /// Clears the boot counter and promotes the pending slot to active. The
    /// application calls this once it has booted successfully, like
    /// embassy-boot's `mark_booted()`.
    ///
    /// # Errors
    ///
    /// Returns the flash driver's error if erase or either write fails.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn mark_booted<F: NorFlash>(&self, flash: &mut F, page_size: u32) -> Result<(), F::Error> {
        let mut state = self.read();
        state.boot_counter = 0;
        if state.pending_slot != state.active_slot {
            state.active_slot = state.pending_slot;
        }
        self.write_boot_state(flash, &state, page_size)
    }

    /// Makes `bank_slot` the pending slot booted on the next reset and clears
    /// the boot counter.
    ///
    /// # Errors
    ///
    /// Returns the flash driver's error if erase or either write fails.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn mark_bank_updated<F: NorFlash>(
        &self,
        flash: &mut F,
        bank_slot: u8,
        page_size: u32,
    ) -> Result<(), F::Error> {
        let mut state = self.read();
        state.pending_slot = bank_slot;
        state.boot_counter = 0;
        self.write_boot_state(flash, &state, page_size)
    }

    /// Called by the bootloader each time it boots the pending slot. Saturates
    /// at 255.
    ///
    /// # Errors
    ///
    /// Returns the flash driver's error if erase or either write fails.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn increment_boot_counter<F: NorFlash>(
        &self,
        flash: &mut F,
        page_size: u32,
    ) -> Result<(), F::Error> {
        let mut state = self.read();
        state.boot_counter = state.boot_counter.saturating_add(1);
        self.write_boot_state(flash, &state, page_size)
    }

    /// True if a pending slot has been booted `max_attempts` times without a
    /// [`Self::mark_booted`].
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn needs_rollback(&self, max_attempts: u8) -> bool {
        let state = self.read();
        state.pending_slot != state.active_slot && state.boot_counter >= max_attempts
    }

    /// Abandons the pending slot by setting it back to the active slot and
    /// clears the boot counter.
    ///
    /// # Errors
    ///
    /// Returns the flash driver's error if erase or either write fails.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn rollback<F: NorFlash>(&self, flash: &mut F, page_size: u32) -> Result<(), F::Error> {
        let mut state = self.read();
        state.pending_slot = state.active_slot;
        state.boot_counter = 0;
        self.write_boot_state(flash, &state, page_size)
    }
}

/// Writes app and network-core update commands into the shared boot-state page.
///
/// `pending_slot` is `Some` only when an application image was staged. Network
/// core commands remain in flash for bootloaders that read the NETD record.
pub fn write_ota_state<F: NorFlash>(
    state_addr: u32,
    flash: &mut F,
    pending_slot: Option<u8>,
    netcore: Option<(u32, u32)>,
    page_size: u32,
) -> Result<(), F::Error> {
    let manager = BootStateManager::new(state_addr);
    let state = manager.read();

    write_ota_state_page(state_addr, flash, state, pending_slot, netcore, page_size)
}

fn write_ota_state_page<F: NorFlash>(
    state_addr: u32,
    flash: &mut F,
    mut state: BootState,
    pending_slot: Option<u8>,
    netcore: Option<(u32, u32)>,
    page_size: u32,
) -> Result<(), F::Error> {
    state.pending_slot = pending_slot.unwrap_or(state.active_slot);
    state.boot_counter = 0;
    let page_start = state_addr & !(page_size - 1);
    let page_end = page_start + page_size;
    flash.erase(page_start, page_end)?;

    // SAFETY: BootState has repr(C), alignment 4, and exactly eight initialized bytes.
    let state_bytes = unsafe {
        core::slice::from_raw_parts(
            &state as *const BootState as *const u8,
            size_of::<BootState>(),
        )
    };
    flash.write(state_addr + 4, state_bytes)?;

    if let Some((address, size)) = netcore {
        #[repr(align(4))]
        struct AlignedRecord([u8; 16]);
        let mut record = AlignedRecord([0xff; 16]);
        record.0[..4].copy_from_slice(&0x4e45_5444_u32.to_le_bytes());
        record.0[4..8].copy_from_slice(&address.to_le_bytes());
        record.0[8..12].copy_from_slice(&size.to_le_bytes());
        flash.write(state_addr + 12, &record.0)?;
    }

    #[repr(align(4))]
    struct AlignedMagic([u8; 4]);
    let magic = AlignedMagic(BOOT_MAGIC.to_le_bytes());
    flash.write(state_addr, &magic.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_corrupt_magic_and_out_of_range_banks() {
        for active_slot in 0..=1 {
            for pending_slot in 0..=1 {
                let state = BootState {
                    active_slot,
                    pending_slot,
                    ..BootState::new()
                };
                assert!(state.is_valid());
            }
        }
        for invalid_slot in [2, u8::MAX] {
            let mut state = BootState::new();
            state.active_slot = invalid_slot;
            assert!(!state.is_valid());
            state.active_slot = 0;
            state.pending_slot = invalid_slot;
            assert!(!state.is_valid());
        }
        let mut state = BootState::new();
        state.magic ^= 1;
        assert!(!state.is_valid());
    }

    #[test]
    fn validates_vectors_and_selects_only_valid_banks() {
        for (sp, reset, expected) in [
            (0x2008_0000, 0x10001, true),
            (0x2000_0000, 0x1ffff, true),
            (0x2000_0004, 0x10001, false),
            (0x2008_0000, 0x10000, false),
            (0x2008_0000, 0x20001, false),
        ] {
            assert_eq!(valid_vectors(0x10000..0x20000, sp, reset), expected);
        }

        let mut state = BootState::new();
        state.pending_slot = 1;
        assert_eq!(select_boot_slot(&mut state, [true, true]), Some(1));
        assert_eq!(state.boot_counter, 1);

        state.boot_counter = 2;
        assert_eq!(select_boot_slot(&mut state, [true, true]), Some(0));
        assert_eq!(state.pending_slot, 0);
        assert_eq!(state.boot_counter, 0);

        state.active_slot = 1;
        state.pending_slot = 1;
        assert_eq!(select_boot_slot(&mut state, [true, false]), Some(0));
        assert_eq!(select_boot_slot(&mut state, [false, false]), None);

        state.active_slot = 0;
        state.pending_slot = 1;
        assert_eq!(select_boot_slot(&mut state, [true, false]), Some(0));
        assert_eq!(state.pending_slot, 0);
        assert_eq!(state.boot_counter, 0);
    }
    struct OtaFlash {
        bytes: [u8; 4096],
        writes: [u32; 3],
        write_count: usize,
        fail_write: Option<usize>,
    }

    impl OtaFlash {
        fn new(fail_write: Option<usize>) -> Self {
            Self {
                bytes: [0xff; 4096],
                writes: [0; 3],
                write_count: 0,
                fail_write,
            }
        }
    }

    impl embedded_storage::nor_flash::ErrorType for OtaFlash {
        type Error = embedded_storage::nor_flash::NorFlashErrorKind;
    }

    impl embedded_storage::nor_flash::ReadNorFlash for OtaFlash {
        const READ_SIZE: usize = 1;

        fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            let start = offset
                .checked_sub(0x1000)
                .ok_or(embedded_storage::nor_flash::NorFlashErrorKind::OutOfBounds)?
                as usize;
            let end = start
                .checked_add(bytes.len())
                .filter(|end| *end <= self.bytes.len())
                .ok_or(embedded_storage::nor_flash::NorFlashErrorKind::OutOfBounds)?;
            bytes.copy_from_slice(&self.bytes[start..end]);
            Ok(())
        }

        fn capacity(&self) -> usize {
            self.bytes.len()
        }
    }

    impl NorFlash for OtaFlash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = 4096;

        fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            if from != 0x1000 || to != 0x2000 {
                return Err(embedded_storage::nor_flash::NorFlashErrorKind::OutOfBounds);
            }
            self.bytes.fill(0xff);
            Ok(())
        }

        fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            let call = self.write_count;
            if bytes.as_ptr() as usize % 4 != 0 {
                return Err(embedded_storage::nor_flash::NorFlashErrorKind::NotAligned);
            }
            self.write_count += 1;
            self.writes[call] = offset;
            if self.fail_write == Some(call) {
                return Err(embedded_storage::nor_flash::NorFlashErrorKind::Other);
            }
            let start = offset
                .checked_sub(0x1000)
                .ok_or(embedded_storage::nor_flash::NorFlashErrorKind::OutOfBounds)?
                as usize;
            let end = start
                .checked_add(bytes.len())
                .filter(|end| *end <= self.bytes.len())
                .ok_or(embedded_storage::nor_flash::NorFlashErrorKind::OutOfBounds)?;
            self.bytes[start..end].copy_from_slice(bytes);
            Ok(())
        }
    }

    #[test]
    fn ota_state_page_handoff_commits_netd_before_activation_magic() {
        use embedded_storage::nor_flash::NorFlashErrorKind;

        let state = BootState {
            active_slot: 1,
            pending_slot: 0,
            boot_counter: 7,
            ..BootState::new()
        };
        let mut flash = OtaFlash::new(None);
        assert!(write_ota_state_page(
            0x1000,
            &mut flash,
            state,
            Some(0),
            Some((0x0100_f000, 0x1234)),
            4096,
        )
        .is_ok());
        assert_eq!(flash.writes, [0x1004, 0x100c, 0x1000]);
        assert_eq!(&flash.bytes[..4], &BOOT_MAGIC.to_le_bytes());
        let mut expected_state = [0; 8];
        expected_state[..4].copy_from_slice(&STATE_MAGIC.to_le_bytes());
        expected_state[4..].copy_from_slice(&[1, 0, 0, 0]);
        assert_eq!(&flash.bytes[4..12], &expected_state);
        let mut expected_netcore = [0; 12];
        expected_netcore[..4].copy_from_slice(&0x4e45_5444_u32.to_le_bytes());
        expected_netcore[4..8].copy_from_slice(&0x0100_f000_u32.to_le_bytes());
        expected_netcore[8..].copy_from_slice(&0x1234_u32.to_le_bytes());
        assert_eq!(&flash.bytes[12..24], &expected_netcore);

        let mut flash = OtaFlash::new(None);
        assert!(write_ota_state_page(0x1000, &mut flash, state, None, None, 4096).is_ok());
        assert_eq!(flash.writes, [0x1004, 0x1000, 0]);
        assert_eq!(flash.bytes[9], 1);
        assert_eq!(&flash.bytes[..4], &BOOT_MAGIC.to_le_bytes());


        let mut flash = OtaFlash::new(Some(2));
        assert_eq!(
            write_ota_state_page(
                0x1000,
                &mut flash,
                state,
                Some(0),
                Some((0x0100_f000, 0x1234)),
                4096,
            ),
            Err(NorFlashErrorKind::Other)
        );
        assert_eq!(&flash.bytes[..4], &[0xff; 4]);
        assert_eq!(flash.writes[2], 0x1000);
    }
}
