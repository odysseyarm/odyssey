#![expect(
    unsafe_code,
    reason = "boot state address comes from linker-defined flash symbols"
)]
#![no_std]

use core::sync::atomic::{AtomicU8, Ordering};

/// Since: Odyssey Desktop 0.7.0.
pub use better_boot::{BootState, BootStateManager};
use embassy_nrf::nvmc::Nvmc;

const PAGE_SIZE: u32 = 4096;

// Boot state as last confirmed, kept in RAM so a DFU request does not re-read
// flash: the flash copy can be corrupted between confirmation and the request.
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
    /// Marks the running image as booted in flash and caches the resulting
    /// slots for [`Self::request_dfu_with`]. Call once the app has come up.
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

    /// Requests DFU by writing SWAP_MAGIC to the flash boot state, keeping the
    /// active and pending slots cached by the last confirmation.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn request_dfu_with(nvmc: &mut Nvmc) -> Result<(), ()> {
        let manager = boot_state_manager();

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
