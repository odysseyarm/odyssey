#![no_std]
#![no_main]

use core::ptr;
use cortex_m_rt::{entry, exception};
#[cfg(feature = "defmt")]
use defmt_rtt as _;
use embassy_nrf::{bind_interrupts, nvmc::Nvmc, peripherals};
use embedded_storage::nor_flash::NorFlash;

// Boot magic values for image validation
const BOOT_MAGIC: [u8; 4] = [0x77, 0xc2, 0x95, 0xf3];
const BOOT_FLAG_BAD: u8 = 0x00;

// A/B DirectXIP boot state magic
const STATE_MAGIC: u32 = 0xB00710AD; // "BOOTLOAD"

// Image header structure (compatible with embassy-boot)
#[repr(C)]
struct ImageHeader {
    magic: [u8; 4],
    flags: u8,
    _reserved: [u8; 3],
    version: u32,
}

// Boot state stored in BOOTLOADER_STATE partition
// This tracks which slot to boot and handles rollback
#[repr(C)]
struct BootState {
    magic: u32,
    /// Which slot booted last time (0 = slot A @ 0x00007000, 1 = slot B @ 0x0005C000)
    active_slot: u8,
    /// Which slot to try booting (for pending update confirmation)
    pending_slot: u8,
    /// Boot attempt counter (incremented on boot, cleared on confirm)
    boot_counter: u8,
    _reserved: u8,
}

extern "C" {
    static __stage2_bootloader_start: u32;
    static __bootloader_state_start: u32;
    static __app_bank_a_start: u32; // Slot A (bank A)
    static __app_bank_b_start: u32; // Slot B (bank B)
}

#[entry]
fn main() -> ! {
    let p = embassy_nrf::init(Default::default());

    // Get addresses from linker symbols
    let stage2_addr = unsafe { &__stage2_bootloader_start as *const u32 as usize };
    let state_addr = unsafe { &__bootloader_state_start as *const u32 as usize };
    let slot_a_addr = unsafe { &__app_bank_a_start as *const u32 as usize };
    let slot_b_addr = unsafe { &__app_bank_b_start as *const u32 as usize };

    // Read boot state
    let mut state = read_boot_state(state_addr);

    // Determine which slot to boot based on A/B DirectXIP logic
    let (boot_addr, booted_slot) =
        select_boot_slot(&mut state, slot_a_addr, slot_b_addr, stage2_addr);

    // Update boot state: increment boot counter for rollback detection
    if boot_addr != stage2_addr {
        state.boot_counter = state.boot_counter.saturating_add(1);

        // Rollback after 1 failed boot (boot_counter == 2 means booted once, failed to confirm)
        if state.boot_counter >= 2 {
            // Rollback: switch to the other slot
            state.pending_slot = if booted_slot == 0 { 1 } else { 0 };
            state.active_slot = state.pending_slot;
            state.boot_counter = 0;

            // Write updated state and retry
            write_boot_state(state_addr, &state, p.NVMC);

            // Boot from the rollback slot
            let rollback_addr = if state.active_slot == 0 {
                slot_a_addr
            } else {
                slot_b_addr
            };

            if let Some(addr) = check_and_validate_image(rollback_addr, false) {
                unsafe {
                    boot_from_address(addr);
                }
            }

            // If rollback also fails, boot stage 2
            unsafe {
                boot_from_address(stage2_addr);
            }
        }

        // Update state with new boot counter
        write_boot_state(state_addr, &state, p.NVMC);
    }

    unsafe {
        boot_from_address(boot_addr);
    }
}

/// Select which slot to boot based on A/B DirectXIP state
fn select_boot_slot(
    state: &mut BootState,
    slot_a: usize,
    slot_b: usize,
    fallback: usize,
) -> (usize, u8) {
    // Priority:
    // 1. Pending slot (if set and valid) - for trying new firmware
    // 2. Active slot (if valid) - normal boot
    // 3. Other slot (if valid) - fallback
    // 4. Stage 2 bootloader - recovery

    if state.pending_slot != state.active_slot {
        // There's a pending update to try
        let pending_addr = if state.pending_slot == 0 {
            slot_a
        } else {
            slot_b
        };
        if let Some(addr) = check_and_validate_image(pending_addr, false) {
            return (addr, state.pending_slot);
        }
    }

    // Try active slot
    let active_addr = if state.active_slot == 0 {
        slot_a
    } else {
        slot_b
    };
    if let Some(addr) = check_and_validate_image(active_addr, false) {
        return (addr, state.active_slot);
    }

    // Try the other slot as fallback
    let other_slot = if state.active_slot == 0 { 1 } else { 0 };
    let other_addr = if other_slot == 0 { slot_a } else { slot_b };
    if let Some(addr) = check_and_validate_image(other_addr, false) {
        return (addr, other_slot);
    }

    // No valid firmware, boot stage 2
    (fallback, 255) // 255 = invalid slot marker
}

/// Read boot state from flash
fn read_boot_state(addr: usize) -> BootState {
    unsafe {
        let state_ptr = addr as *const BootState;
        let state = ptr::read_volatile(state_ptr);

        // Validate magic and return default if corrupted
        if state.magic == STATE_MAGIC {
            state
        } else {
            // Initialize default state: boot from slot A
            BootState {
                magic: STATE_MAGIC,
                active_slot: 0, // Start with slot A
                pending_slot: 0,
                boot_counter: 0,
                _reserved: 0,
            }
        }
    }
}

/// Write boot state to flash using NVMC
fn write_boot_state(
    addr: usize,
    state: &BootState,
    nvmc: embassy_nrf::Peri<'static, peripherals::NVMC>,
) {
    use embedded_storage::nor_flash::NorFlash;

    let mut nvmc = Nvmc::new(nvmc);
    const PAGE_SIZE: usize = 4096;

    unsafe {
        // Erase the state page
        nvmc.erase(addr as u32, (addr + PAGE_SIZE) as u32).unwrap();

        // Write the state
        let src = state as *const BootState as *const u8;
        let src_slice = core::slice::from_raw_parts(src, core::mem::size_of::<BootState>());
        nvmc.write(addr as u32, src_slice).unwrap();
    }
}

/// Check if an image at the given address is valid and bootable
/// Returns Some(addr) if valid, None otherwise
fn check_and_validate_image(addr: usize, require_boot_flag: bool) -> Option<usize> {
    let header_ptr = addr as *const ImageHeader;

    // Basic sanity check - ensure we're reading from flash
    if addr < 0x1000 || addr > 0x00100000 {
        return None;
    }

    unsafe {
        let header = &*header_ptr;

        // Check magic number
        if header.magic != BOOT_MAGIC {
            return None;
        }

        // Check if marked as bad
        if header.flags == BOOT_FLAG_BAD {
            return None;
        }

        // Validate vector table - check stack pointer is in RAM
        let vtor_ptr = (addr + core::mem::size_of::<ImageHeader>()) as *const u32;
        let stack_ptr = *vtor_ptr;

        // Stack should be in RAM (0x20000000 - 0x20080000)
        if stack_ptr < 0x20000000 || stack_ptr > 0x20080000 {
            return None;
        }

        // Reset vector should have thumb bit set
        let reset_vector = *vtor_ptr.offset(1);
        if (reset_vector & 0x1) == 0 {
            return None;
        }

        Some(addr + core::mem::size_of::<ImageHeader>())
    }
}

/// Boot from the specified address
/// This function does not return
unsafe fn boot_from_address(addr: usize) -> ! {
    let mut p = cortex_m::Peripherals::steal();

    cortex_m::interrupt::disable();

    // Invalidate caches
    p.SCB.invalidate_icache();

    // Set vector table offset
    p.SCB.vtor.write(addr as u32);

    // Enable interrupts
    cortex_m::interrupt::enable();

    // Boot to the new image
    cortex_m::asm::bootload(addr as *const u32)
}

#[exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
    let irqn = core::ptr::read_volatile(SCB_ICSR) as u8 as i16 - 16;

    panic!("DefaultHandler #{:?}", irqn);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // In a panic, try to boot the second-stage bootloader
    let stage2_addr = unsafe { &__stage2_bootloader_start as *const u32 as usize };
    unsafe {
        boot_from_address(stage2_addr);
    }
}
