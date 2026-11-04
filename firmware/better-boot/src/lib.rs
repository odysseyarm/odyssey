//! Dual-bank boot state kept at a flash address chosen at runtime, instead of
//! the fixed linker symbols embassy-boot requires.

#![no_std]

use core::ptr;

use embedded_storage::nor_flash::NorFlash;

/// Value of [`BootState::magic`] once the state has been written. Specific to
/// this crate; embassy-boot has no equivalent.
pub const STATE_MAGIC: u32 = 0xB00710AD;
/// Partition magic word for a normal boot. Same value as embassy-boot.
pub const BOOT_MAGIC: u32 = 0xD0D0D0D0;
/// Partition magic word that requests DFU mode. Same value as embassy-boot's
/// swap magic.
pub const SWAP_MAGIC: u32 = 0xF0F0F0F0;

/// Boot state stored in flash right after the 4-byte partition magic word.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BootState {
    /// [`STATE_MAGIC`] if the state was written by this crate; anything else
    /// means erased or corrupt flash.
    pub magic: u32,
    /// Currently active slot (0 = Bank A, 1 = Bank B)
    pub active_slot: u8,
    /// Pending slot to boot next (if different from active)
    pub pending_slot: u8,
    /// Incremented by the bootloader each time it boots the pending slot.
    pub boot_counter: u8,
    /// Unused; always written as 0.
    pub reserved: u8,
}

impl BootState {
    /// Both slots on Bank A, boot counter at zero.
    pub fn new() -> Self {
        Self {
            magic: STATE_MAGIC,
            active_slot: 0,
            pending_slot: 0,
            boot_counter: 0,
            reserved: 0,
        }
    }

    /// True if the magic matches [`STATE_MAGIC`] rather than erased flash or
    /// garbage.
    pub fn is_valid(&self) -> bool {
        self.magic == STATE_MAGIC
    }
}

impl Default for BootState {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads and writes the boot state partition at `state_addr`: a 4-byte magic
/// word ([`BOOT_MAGIC`] or [`SWAP_MAGIC`]) followed by a [`BootState`].
pub struct BootStateManager {
    state_addr: u32,
}

impl BootStateManager {
    /// `state_addr` must be memory-mapped flash: [`Self::read`] and
    /// [`Self::read_magic`] dereference it directly.
    pub const fn new(state_addr: u32) -> Self {
        Self { state_addr }
    }

    /// Reads the state stored after the magic word, or [`BootState::new`] if
    /// its magic is not [`STATE_MAGIC`].
    pub fn read(&self) -> BootState {
        unsafe {
            let state_ptr =
                (self.state_addr as usize + core::mem::size_of::<u32>()) as *const BootState;
            let state = ptr::read_volatile(state_ptr);

            if state.is_valid() {
                state
            } else {
                BootState::new()
            }
        }
    }

    /// Reads the magic word at the start of the state partition.
    pub fn read_magic(&self) -> u32 {
        unsafe { ptr::read_volatile(self.state_addr as *const u32) }
    }

    /// Erases the flash page holding the state, then writes [`BOOT_MAGIC`] and
    /// `state`. `page_size` must be a power of two.
    ///
    /// # Errors
    ///
    /// Returns the flash driver's error if the erase or a write fails.
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

        // SAFETY: BootState is repr(C) with only integer fields and no padding,
        // so all of its bytes are initialised.
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

        // SAFETY: BootState is repr(C) with only integer fields and no padding,
        // so all of its bytes are initialised.
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
    pub fn is_dfu_requested(&self) -> bool {
        self.read_magic() == SWAP_MAGIC
    }

    /// Clears the boot counter and promotes the pending slot to active. The
    /// application calls this once it has booted successfully; equivalent to
    /// embassy-boot's `mark_booted()`.
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
    pub fn needs_rollback(&self, max_attempts: u8) -> bool {
        let state = self.read();
        state.pending_slot != state.active_slot && state.boot_counter >= max_attempts
    }

    /// Abandons the pending slot by setting it back to the active slot and
    /// clears the boot counter.
    pub fn rollback<F: NorFlash>(&self, flash: &mut F, page_size: u32) -> Result<(), F::Error> {
        let mut state = self.read();
        state.pending_slot = state.active_slot;
        state.boot_counter = 0;
        self.write_boot_state(flash, &state, page_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_boot_state_new() {
        let state = BootState::new();
        assert!(state.is_valid());
        assert_eq!(state.active_slot, 0);
        assert_eq!(state.pending_slot, 0);
        assert_eq!(state.boot_counter, 0);
    }
}
