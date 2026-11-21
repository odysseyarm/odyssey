//! Boot state shared between the lite1 application and its bootloader.

#![no_std]
#![expect(
    unsafe_code,
    reason = "Boot-state linker symbol exposes fixed flash address"
)]

use core::sync::atomic::{AtomicU8, Ordering};

/// Since: Odyssey Desktop 0.7.0.
pub use better_boot::{BootState, BootStateManager};
use embassy_nrf::nvmc::Nvmc;

const PAGE_SIZE: u32 = 4096;

// Slots as of the last confirm, cached so `request_dfu_with` does not re-read
// flash: the state page can be corrupted between confirm and the DFU request.
static CACHED_ACTIVE_SLOT: AtomicU8 = AtomicU8::new(0);
static CACHED_PENDING_SLOT: AtomicU8 = AtomicU8::new(0);

fn boot_state_manager() -> BootStateManager {
    unsafe extern "C" {
        static __bootloader_state_start: u32;
        static __bootloader_journal_start: u32;
    }
    BootStateManager::new(
        &raw const __bootloader_state_start as u32,
        &raw const __bootloader_journal_start as u32,
    )
}

fn running_slot() -> u8 {
    unsafe extern "C" {
        static __bootloader_app_b_start: u32;
    }
    u8::from(running_slot as *const () as usize >= &raw const __bootloader_app_b_start as usize)
}

/// Since: Odyssey Desktop 0.7.0.
pub struct BootConfirmation;

impl BootConfirmation {
    /// Marks the current boot as successful in flash and caches the resulting
    /// slots for [`Self::request_dfu_with`]. The application calls this once it
    /// has booted.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn mark_booted(nvmc: &mut Nvmc) -> Result<(), ()> {
        let manager = boot_state_manager();

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

        manager
            .mark_running_booted(nvmc, running_slot(), PAGE_SIZE)
            .map_err(|_| ())?;

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

    /// Confirms the running image after its startup checks pass.
    pub fn confirm_boot_with(nvmc: &mut Nvmc) -> Result<(), ()> {
        Self::mark_booted(nvmc)
    }

    /// Requests DFU by writing SWAP_MAGIC to the flash state page, keeping the
    /// active and pending slots cached at the last confirm.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn request_dfu_with(nvmc: &mut Nvmc) -> Result<(), ()> {
        let manager = boot_state_manager();

        let active_slot = CACHED_ACTIVE_SLOT.load(Ordering::Acquire);
        let pending_slot = CACHED_PENDING_SLOT.load(Ordering::Acquire);

        let state = BootState {
            magic: 0xB00710AD, // better_boot STATE_MAGIC
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
