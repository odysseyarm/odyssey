#![no_std]

pub use better_boot::{BootState, BootStateManager};
use core::sync::atomic::{AtomicU8, Ordering};
use embassy_nrf::nvmc::Nvmc;

const PAGE_SIZE: u32 = 4096;

// Cache the confirmed boot state in RAM so we don't have to re-read from flash
// This avoids issues where flash might be corrupted between confirm and DFU request
static CACHED_ACTIVE_SLOT: AtomicU8 = AtomicU8::new(0);
static CACHED_PENDING_SLOT: AtomicU8 = AtomicU8::new(0);

fn bootloader_state_addr() -> u32 {
    extern "C" {
        static __bootloader_state_start: u32;
    }
    unsafe { &__bootloader_state_start as *const u32 as u32 }
}

pub struct BootConfirmation;

impl BootConfirmation {
    /// Mark boot as successful - should be called by app after successful boot
    /// This updates the cached state for DFU requests
    pub fn mark_booted(nvmc: &mut Nvmc) -> Result<(), ()> {
        let manager = BootStateManager::new(bootloader_state_addr());

        #[cfg(feature = "defmt")]
        {
            let state = manager.read();
            defmt::info!(
                "[boot-api] mark_booted: before - active={}, pending={}, boot_counter={}",
                state.active_slot,
                state.pending_slot,
                state.boot_counter
            );
        }

        // Call mark_booted to update flash
        manager.mark_booted(nvmc, PAGE_SIZE).map_err(|_| ())?;

        // Read back and cache the updated state
        let state = manager.read();
        CACHED_ACTIVE_SLOT.store(state.active_slot, Ordering::Release);
        CACHED_PENDING_SLOT.store(state.pending_slot, Ordering::Release);

        #[cfg(feature = "defmt")]
        defmt::info!(
            "[boot-api] mark_booted: after - active={}, pending={}, boot_counter={} (cached)",
            state.active_slot,
            state.pending_slot,
            state.boot_counter
        );

        Ok(())
    }

    /// Confirm boot - marks the current boot as successful
    /// This sets boot_counter to 0 and updates active_slot to pending_slot
    pub fn confirm_boot_with(nvmc: &mut Nvmc) -> Result<(), ()> {
        let manager = BootStateManager::new(bootloader_state_addr());
        let mut state = manager.read();

        #[cfg(feature = "defmt")]
        defmt::info!(
            "[boot-api] confirm_boot: read state active={}, pending={}, boot_counter={}",
            state.active_slot,
            state.pending_slot,
            state.boot_counter
        );

        state.boot_counter = 0;
        if state.pending_slot != state.active_slot {
            state.active_slot = state.pending_slot;
        }

        #[cfg(feature = "defmt")]
        defmt::info!(
            "[boot-api] confirm_boot: writing state active={}, pending={}, boot_counter={}",
            state.active_slot,
            state.pending_slot,
            state.boot_counter
        );

        // Cache the confirmed state in RAM for later use by request_dfu_with
        CACHED_ACTIVE_SLOT.store(state.active_slot, Ordering::Release);
        CACHED_PENDING_SLOT.store(state.pending_slot, Ordering::Release);

        let result = manager
            .write_boot_state(nvmc, &state, PAGE_SIZE)
            .map_err(|_| ());

        // Verify what was actually written
        #[cfg(feature = "defmt")]
        {
            let verify = manager.read();
            let magic = manager.read_magic();
            defmt::info!(
                "[boot-api] confirm_boot: verified magic={:#010x}, active={}, pending={}, boot_counter={}",
                magic,
                verify.active_slot,
                verify.pending_slot,
                verify.boot_counter
            );
        }

        result
    }

    /// Request DFU by writing SWAP_MAGIC to flash state
    /// This preserves the boot state (active/pending slots)
    pub fn request_dfu_with(nvmc: &mut Nvmc) -> Result<(), ()> {
        let manager = BootStateManager::new(bootloader_state_addr());

        // Use cached state from RAM instead of re-reading flash
        // Flash might be corrupted between boot confirmation and DFU request
        let active_slot = CACHED_ACTIVE_SLOT.load(Ordering::Acquire);
        let pending_slot = CACHED_PENDING_SLOT.load(Ordering::Acquire);

        let state = BootState {
            magic: 0xB00710AD, // STATE_MAGIC
            active_slot,
            pending_slot,
            boot_counter: 0,
            reserved: 0,
        };

        #[cfg(feature = "defmt")]
        defmt::info!(
            "[boot-api] request_dfu_with: using cached state active={}, pending={}, boot_counter={}",
            state.active_slot,
            state.pending_slot,
            state.boot_counter
        );

        let result = manager.request_dfu(nvmc, &state, PAGE_SIZE).map_err(|_| ());

        // Verify what was written
        #[cfg(feature = "defmt")]
        {
            let verification = manager.read();
            defmt::info!(
                "[boot-api] after request_dfu: active={}, pending={}, boot_counter={}",
                verification.active_slot,
                verification.pending_slot,
                verification.boot_counter
            );
        }

        result
    }
}
