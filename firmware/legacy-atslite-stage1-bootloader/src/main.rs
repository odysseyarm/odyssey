#![expect(
    unsafe_code,
    reason = "stage-one boot entry transfers control to linked application image"
)]
#![no_std]
#![no_main]

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

    // With RTT attached, uncomment this delay: flash accessed too soon after boot faults.
    // for _ in 0..10000000 {
    //     cortex_m::asm::nop();
    // }

    // Stage1 starts no watchdog. Stage2 does, so a stage2 update can change
    // the watchdog configuration.
    let flash = Nvmc::new(p.NVMC);
    let flash = Mutex::<NoopRawMutex, _>::new(RefCell::new(flash));

    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();

    // Swaps in a pending update and rolls back failed boots.
    let bl: BootLoader = BootLoader::prepare(config);

    // SAFETY: BootLoader::prepare validates both slots and linker config provides aligned executable active offset.
    unsafe { bl.load(active_offset) }
}

#[unsafe(no_mangle)]
#[cfg_attr(target_os = "none", unsafe(link_section = ".HardFault.user"))]
/// # Safety
/// Invoked only by the Cortex-M HardFault vector with interrupts already disabled.
unsafe extern "C" fn HardFault() {
    cortex_m::peripheral::SCB::sys_reset();
}

#[exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    // SAFETY: exception handler runs in privileged mode; ICSR is the fixed, readable Cortex-M system register.
    unsafe {
        const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
        let irqn = core::ptr::read_volatile(SCB_ICSR) as u8 as i16 - 16;
        panic!("DefaultHandler #{:?}", irqn);
    }
}
