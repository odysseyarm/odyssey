//! Boot state shared between the lite1 application and its bootloader.

#![no_std]

use core::sync::atomic::{AtomicU8, Ordering};

pub use better_boot::{BootState, BootStateManager};
use embassy_nrf::nvmc::Nvmc;

const PAGE_SIZE: u32 = 4096;

// Slots as of the last confirm, cached so `request_dfu_with` does not re-read
// flash: the state page can be corrupted between confirm and the DFU request.
static CACHED_ACTIVE_SLOT: AtomicU8 = AtomicU8::new(0);
static CACHED_PENDING_SLOT: AtomicU8 = AtomicU8::new(0);

fn bootloader_state_addr() -> u32 {
    unsafe extern "C" {
        static __bootloader_state_start: u32;
    }
    unsafe { &__bootloader_state_start as *const u32 as u32 }
}

pub struct BootConfirmation;

impl BootConfirmation {
    /// Marks the current boot as successful in flash and caches the resulting
    /// slots for [`Self::request_dfu_with`]. The application calls this once it
    /// has booted.
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

        manager.mark_booted(nvmc, PAGE_SIZE).map_err(|_| ())?;

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

    /// Confirms the current boot: sets `boot_counter` to 0, makes
    /// `pending_slot` the active slot, and caches the slots for
    /// [`Self::request_dfu_with`].
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

        CACHED_ACTIVE_SLOT.store(state.active_slot, Ordering::Release);
        CACHED_PENDING_SLOT.store(state.pending_slot, Ordering::Release);

        let result = manager
            .write_boot_state(nvmc, &state, PAGE_SIZE)
            .map_err(|_| ());

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

    /// Requests DFU by writing SWAP_MAGIC to the flash state page, keeping the
    /// active and pending slots cached at the last confirm.
    pub fn request_dfu_with(nvmc: &mut Nvmc) -> Result<(), ()> {
        let manager = BootStateManager::new(bootloader_state_addr());

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
