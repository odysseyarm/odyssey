#![no_std]

use core::ptr;
use core::sync::atomic::{AtomicU8, Ordering};
use embassy_nrf::nvmc::Nvmc;
use embedded_storage::nor_flash::NorFlash;

const STATE_MAGIC: u32 = 0xB00710AD;
const BOOT_MAGIC: u32 = 0xB0070000;
const SWAP_MAGIC: u32 = 0x00BAD00D;

// Cache the confirmed boot state in RAM so we don't have to re-read from flash
// This avoids issues where flash might be corrupted between confirm and DFU request
static CACHED_ACTIVE_SLOT: AtomicU8 = AtomicU8::new(0);
static CACHED_PENDING_SLOT: AtomicU8 = AtomicU8::new(0);

fn bootloader_state_addr() -> usize {
    extern "C" {
        static __bootloader_state_start: u32;
    }
    unsafe { &__bootloader_state_start as *const u32 as usize }
}

#[repr(C)]
struct BootState {
    magic: u32,
    active_slot: u8,
    pending_slot: u8,
    boot_counter: u8,
    _reserved: u8,
}

pub struct BootConfirmation;

impl BootConfirmation {
    pub fn confirm_boot_with(nvmc: &mut Nvmc) -> Result<(), ()> {
        let mut state = read_boot_state();
        state.boot_counter = 0;
        if state.pending_slot != state.active_slot {
            state.active_slot = state.pending_slot;
        }

        // Cache the confirmed state in RAM for later use by request_dfu_with
        CACHED_ACTIVE_SLOT.store(state.active_slot, Ordering::Release);
        CACHED_PENDING_SLOT.store(state.pending_slot, Ordering::Release);

        write_boot_state_with(&state, nvmc)
    }

    /// Request DFU by writing SWAP_MAGIC to flash state
    /// This preserves the boot state (active/pending slots)
    pub fn request_dfu_with(nvmc: &mut Nvmc) -> Result<(), ()> {
        // Use cached state from RAM instead of re-reading flash
        // Flash might be corrupted between boot confirmation and DFU request
        let active_slot = CACHED_ACTIVE_SLOT.load(Ordering::Acquire);
        let pending_slot = CACHED_PENDING_SLOT.load(Ordering::Acquire);

        let state = BootState {
            magic: STATE_MAGIC,
            active_slot,
            pending_slot,
            boot_counter: 0,
            _reserved: 0,
        };

        #[cfg(feature = "defmt")]
        defmt::info!(
            "[boot-api] request_dfu_with: using cached state active={}, pending={}, boot_counter={}",
            state.active_slot,
            state.pending_slot,
            state.boot_counter
        );

        // Write SWAP_MAGIC + preserved state
        let result = write_dfu_request(&state, nvmc);

        // Verify what was written
        let verification = read_boot_state_raw();
        #[cfg(feature = "defmt")]
        defmt::info!(
            "[boot-api] after write_dfu_request: active={}, pending={}, boot_counter={}",
            verification.active_slot,
            verification.pending_slot,
            verification.boot_counter
        );

        result
    }
}

fn read_boot_state() -> BootState {
    unsafe {
        let state_ptr = (bootloader_state_addr() + core::mem::size_of::<u32>()) as *const BootState;
        let state = ptr::read_volatile(state_ptr);

        if state.magic == STATE_MAGIC {
            state
        } else {
            BootState {
                magic: STATE_MAGIC,
                active_slot: 0,
                pending_slot: 0,
                boot_counter: 0,
                _reserved: 0,
            }
        }
    }
}

fn read_boot_state_raw() -> BootState {
    unsafe {
        let state_ptr = (bootloader_state_addr() + core::mem::size_of::<u32>()) as *const BootState;
        ptr::read_volatile(state_ptr)
    }
}

fn write_boot_state_with(state: &BootState, nvmc: &mut Nvmc) -> Result<(), ()> {
    use embassy_nrf::nvmc::PAGE_SIZE;

    let base = bootloader_state_addr() as u32;
    nvmc.erase(base, base + PAGE_SIZE as u32).map_err(|_| ())?;
    nvmc.write(base, &BOOT_MAGIC.to_le_bytes())
        .map_err(|_| ())?;

    let bytes = unsafe {
        core::slice::from_raw_parts(
            state as *const BootState as *const u8,
            core::mem::size_of::<BootState>(),
        )
    };
    let dst = (bootloader_state_addr() + core::mem::size_of::<u32>()) as u32;
    nvmc.write(dst, bytes).map_err(|_| ())
}

fn write_dfu_request(state: &BootState, nvmc: &mut Nvmc) -> Result<(), ()> {
    use embassy_nrf::nvmc::PAGE_SIZE;

    let base = bootloader_state_addr() as u32;
    nvmc.erase(base, base + PAGE_SIZE as u32).map_err(|_| ())?;

    // Write SWAP_MAGIC instead of BOOT_MAGIC
    nvmc.write(base, &SWAP_MAGIC.to_le_bytes())
        .map_err(|_| ())?;

    // Write preserved boot state
    let bytes = unsafe {
        core::slice::from_raw_parts(
            state as *const BootState as *const u8,
            core::mem::size_of::<BootState>(),
        )
    };
    let dst = (bootloader_state_addr() + core::mem::size_of::<u32>()) as u32;
    nvmc.write(dst, bytes).map_err(|_| ())
}
