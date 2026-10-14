#![no_std]
#![no_main]

use core::cell::RefCell;

use cortex_m_rt::{entry, exception};
#[cfg(feature = "defmt")]
use defmt_rtt as _;
use embassy_boot_nrf::*;
use embassy_nrf::nvmc::Nvmc;
use embassy_sync::blocking_mutex::{Mutex, raw::NoopRawMutex};
#[cfg(not(feature = "defmt"))]
use panic_halt as _;
#[cfg(feature = "defmt")]
use panic_probe as _;

#[entry]
fn main() -> ! {
    let p = embassy_nrf::init(Default::default());

    // Uncomment this if debugging with RTT attached to prevent early flash access faults
    // for _ in 0..10000000 {
    //     cortex_m::asm::nop();
    // }

    // No watchdog in stage1 - stage2 bootloader starts it instead
    // This allows updating watchdog configuration via stage2 bootloader updates
    let flash = Nvmc::new(p.NVMC);
    let flash = Mutex::<NoopRawMutex, _>::new(RefCell::new(flash));

    // Get bootloader config from linker symbols (ACTIVE, DFU, STATE partitions)
    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();

    // Prepare boot: performs swap if needed, handles rollback on failed boots
    let bl: BootLoader = BootLoader::prepare(config);

    // Boot the stage2 bootloader (with swap performed if needed)
    unsafe { bl.load(active_offset) }
}

#[unsafe(no_mangle)]
#[cfg_attr(target_os = "none", unsafe(link_section = ".HardFault.user"))]
unsafe extern "C" fn HardFault() {
    cortex_m::peripheral::SCB::sys_reset();
}

#[exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
    let irqn = core::ptr::read_volatile(SCB_ICSR) as u8 as i16 - 16;
    panic!("DefaultHandler #{:?}", irqn);
}
