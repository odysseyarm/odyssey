//! Better Boot - Dual-bank boot state management without fixed linker symbols
//!
//! This crate provides a clean API for managing dual-bank firmware boot state,
//! allowing you to specify partition addresses at runtime instead of relying on
//! fixed linker symbols like embassy-boot does.

#![no_std]

use core::ptr;
use embedded_storage::nor_flash::NorFlash;

// Magic constants matching embassy-boot
// BOOT_MAGIC: 0xD0 repeated 4 times = 0xD0D0D0D0
// SWAP_MAGIC: 0xF0 repeated 4 times = 0xF0F0F0F0
pub const STATE_MAGIC: u32 = 0xB00710AD; // Keep our own state magic
pub const BOOT_MAGIC: u32 = 0xD0D0D0D0; // Match embassy-boot
pub const SWAP_MAGIC: u32 = 0xF0F0F0F0; // Match embassy-boot

/// Boot state stored in flash
#[repr(C)]
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BootState {
    /// Magic number to identify valid state
    pub magic: u32,
    /// Currently active slot (0 = Bank A, 1 = Bank B)
    pub active_slot: u8,
    /// Pending slot to boot next (if different from active)
    pub pending_slot: u8,
    /// Boot attempt counter (incremented on each boot of pending slot)
    pub boot_counter: u8,
    /// Reserved for future use
    pub reserved: u8,
}

impl BootState {
    /// Create a new boot state with both slots pointing to Bank A
    pub fn new() -> Self {
        Self {
            magic: STATE_MAGIC,
            active_slot: 0,
            pending_slot: 0,
            boot_counter: 0,
            reserved: 0,
        }
    }

    /// Check if the state magic is valid
    pub fn is_valid(&self) -> bool {
        self.magic == STATE_MAGIC
    }
}

impl Default for BootState {
    fn default() -> Self {
        Self::new()
    }
}

/// Boot state manager that operates on a specific flash address
pub struct BootStateManager {
    state_addr: u32,
}

impl BootStateManager {
    /// Create a new boot state manager for the given flash address
    pub const fn new(state_addr: u32) -> Self {
        Self { state_addr }
    }

    /// Read the current boot state from flash
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

    /// Read the magic word (first 4 bytes) from the state partition
    pub fn read_magic(&self) -> u32 {
        unsafe { ptr::read_volatile(self.state_addr as *const u32) }
    }

    /// Write boot state to flash with BOOT_MAGIC (normal boot)
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

    /// Write DFU request to flash with SWAP_MAGIC (request DFU mode)
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

    /// Check if DFU mode is requested (SWAP_MAGIC is set)
    pub fn is_dfu_requested(&self) -> bool {
        self.read_magic() == SWAP_MAGIC
    }

    /// Mark boot as successful - sets boot_counter to 0 and updates active_slot to pending_slot
    /// This should be called by the application after successful boot
    /// This is equivalent to embassy-boot's mark_booted()
    pub fn mark_booted<F: NorFlash>(&self, flash: &mut F, page_size: u32) -> Result<(), F::Error> {
        let mut state = self.read();
        state.boot_counter = 0;
        if state.pending_slot != state.active_slot {
            state.active_slot = state.pending_slot;
        }
        self.write_boot_state(flash, &state, page_size)
    }

    /// Mark bank as updated - sets pending_slot and prepares for boot on next reset
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

    /// Increment boot counter (called by bootloader when booting pending slot)
    pub fn increment_boot_counter<F: NorFlash>(
        &self,
        flash: &mut F,
        page_size: u32,
    ) -> Result<(), F::Error> {
        let mut state = self.read();
        state.boot_counter = state.boot_counter.saturating_add(1);
        self.write_boot_state(flash, &state, page_size)
    }

    /// Check if rollback is needed (boot_counter exceeded threshold)
    pub fn needs_rollback(&self, max_attempts: u8) -> bool {
        let state = self.read();
        state.pending_slot != state.active_slot && state.boot_counter >= max_attempts
    }

    /// Perform rollback - reset pending_slot to active_slot
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
