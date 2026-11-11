#![no_std]
#![no_main]
#![expect(
    unsafe_code,
    reason = "Stage-one boot transfer and exception handlers require raw peripheral access"
)]

use core::cell::RefCell;

use cortex_m_rt::{entry, exception};
#[cfg(feature = "defmt")]
use defmt_rtt as _;
use embassy_boot_nrf::*;
use embassy_nrf::nvmc::Nvmc;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
#[cfg(not(feature = "defmt"))]
use panic_halt as _;
#[cfg(feature = "defmt")]
use panic_probe as _;

#[entry]
fn main() -> ! {
    let p = embassy_nrf::init(Default::default());

    // With RTT attached, touching flash this soon after boot faults; enable this delay when debugging.
    // for _ in 0..10000000 {
    //     cortex_m::asm::nop();
    // }

    // No watchdog here: stage2 starts it, so its configuration can change with a
    // stage2 update.
    let flash = Nvmc::new(p.NVMC);
    let flash = Mutex::<NoopRawMutex, _>::new(RefCell::new(flash));

    // ACTIVE, DFU and STATE partitions come from the linker script.
    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();

    // Swaps in a pending stage2 update, or rolls back after a failed boot.
    let bl: BootLoader = BootLoader::prepare(config);

    // SAFETY: `active_offset` comes from the linker-defined active partition and BootLoader validates its vectors before transfer.
    unsafe { bl.load(active_offset) }
}

/// # Safety
///
/// Called only by the Cortex-M HardFault vector while executing exception context.
#[unsafe(no_mangle)]
#[cfg_attr(target_os = "none", unsafe(link_section = ".HardFault.user"))]
// SAFETY: Exception ABI requires this unmangled symbol; reset is valid from HardFault context.
unsafe extern "C" fn HardFault() {
    cortex_m::peripheral::SCB::sys_reset();
}

/// # Safety
///
/// Called only by the Cortex-M exception vector with the active interrupt state.
#[exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    // SAFETY: SCB_ICSR is the architected, readable interrupt-control register.
    unsafe {
        const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
        let irqn = core::ptr::read_volatile(SCB_ICSR) as u8 as i16 - 16;
        panic!("DefaultHandler #{:?}", irqn);
    }
}
