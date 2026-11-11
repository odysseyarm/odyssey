use embassy_nrf::config::{Config, HfclkSource, LfclkSource};
use embassy_nrf::{Peripherals, pac};

// Arms the Cortex-M33 stack limit (MSPLIM) at the end of .uninit before any Rust frame exists,
// so a stack overflow into statics raises a UsageFault at the `sub sp` that crosses it instead
// of corrupting RAM. Assembly because cortex-m-rt calls `__pre_init` before RAM is initialised.
// MSPLIM ignores bits [2:0], so the limit is rounded up to keep the last .uninit word covered.
core::arch::global_asm!(
    ".section .text.__pre_init,\"ax\"",
    ".global __pre_init",
    ".type __pre_init,%function",
    ".thumb_func",
    "__pre_init:",
    "ldr r0, =_stack_end",
    "adds r0, r0, #7",
    "bic r0, r0, #7",
    "msr MSPLIM, r0",
    "bx lr",
);

/// Opens every flash region, EXTDOMAIN[0] and the ICMSG RX/TX RAM regions to the network core.
unsafe fn configure_spu_for_network_core() {
    use embassy_nrf::pac::spu::vals;

    unsafe extern "C" {
        static __icmsg_tx_start: u32;
        static __icmsg_tx_end: u32;
        static __icmsg_rx_start: u32;
        static __icmsg_rx_end: u32;
    }

    const RAM_BASE: u32 = 0x2000_0000;
    const RAM_REGION_SIZE: u32 = 8 * 1024; // 8 KiB per SPU RAM region (nRF53)
    // Engineering A silicon has 32 flash regions of 32 KiB, later revisions 64 of 16 KiB.
    // Writing 64 covers both.
    const NUM_FLASH_REGIONS: usize = 64;

    #[inline]
    fn to_ram_region_index(addr: u32) -> u32 {
        (addr.saturating_sub(RAM_BASE)) / RAM_REGION_SIZE
    }

    let spu = embassy_nrf::pac::SPU_S;

    // Make all flash non-secure so the network core can reach it: app flash at 0x0000_0000
    // and network core flash at 0x0100_0000.
    for i in 0..NUM_FLASH_REGIONS {
        spu.flashregion(i).perm().write(|w| {
            w.set_read(true);
            w.set_write(true);
            w.set_execute(true);
            w.set_secattr(false);
            w.set_lock(false);
        });
    }

    // EXTDOMAIN[0] is the network core; map it non-secure.
    spu.extdomain(0).perm().write(|w| {
        w.set_securemapping(vals::ExtdomainPermSecuremapping::NON_SECURE);
        w.set_lock(false);
    });

    let tx_start = (&raw const __icmsg_tx_start) as u32;
    let tx_end = (&raw const __icmsg_tx_end) as u32;
    let rx_start = (&raw const __icmsg_rx_start) as u32;
    let rx_end = (&raw const __icmsg_rx_end) as u32;

    let tx_first = to_ram_region_index(tx_start);
    let tx_last = to_ram_region_index(tx_end.saturating_sub(1));
    let rx_first = to_ram_region_index(rx_start);
    let rx_last = to_ram_region_index(rx_end.saturating_sub(1));

    let configure_ram_range = |first: u32, last: u32| {
        for i in first..=last {
            spu.ramregion(i as usize).perm().write(|w| {
                w.set_read(true);
                w.set_write(true);
                w.set_execute(true);
                w.set_secattr(false);
            });
        }
    };

    configure_ram_range(tx_first, tx_last);
    configure_ram_range(rx_first, rx_last);
}

pub fn init() -> (cortex_m::Peripherals, Peripherals) {
    crate::crash_record::report_previous_boot();

    let mut config = Config::default();

    config.dcdc.regh = true;
    // REGMAIN and REGRADIO DC/DC stay off: the erratum 160 workaround is not implemented.
    config.dcdc.regmain = false;
    config.dcdc.regradio = false;
    config.hfclk_source = HfclkSource::ExternalXtal; // the radio needs HFXO
    config.lfclk_source = LfclkSource::ExternalXtal;

    // The BC40P module carries both crystals' load capacitors; internal ones would add to them.
    config.internal_capacitors.hfxo = None;
    config.internal_capacitors.lfxo = None;

    // SPU permissions must be in place before anything else touches the network core.
    unsafe {
        configure_spu_for_network_core();
    }

    pac::CACHE.enable().write(|w| w.set_enable(true));
    // HCLK at 128 MHz (DIV1) avoids erratum 166.
    pac::CLOCK
        .hfclkctrl()
        .write(|w| w.set_hclk(pac::clock::vals::Hclk::DIV1));

    let peripherals = embassy_nrf::init(config);

    // probe-rs fails to attach while the network core is locked or held off:
    // https://github.com/probe-rs/probe-rs/issues/3053
    let mut core_peripherals = cortex_m::Peripherals::take().unwrap();
    let systick_delay = cortex_m::delay::Delay::new(core_peripherals.SYST, 128_000_000);

    core_peripherals.SYST = systick_delay.free();
    (core_peripherals, peripherals)
}

/// Releases the network core from FORCEOFF. Call only after the SPU grants are written.
pub fn start_network_core_after_spu() {
    use embassy_nrf::pac::reset::vals::Forceoff;
    // Erratum 161: hold 0x50005618 at 1 while toggling FORCEOFF.
    unsafe {
        (0x50005618 as *mut u32).write_volatile(1);
    }
    embassy_nrf::pac::RESET
        .network()
        .forceoff()
        .write(|w| w.set_forceoff(Forceoff::RELEASE));
    // The net core runs from its own clock, so a short busy-wait is enough between toggles.
    cortex_m::asm::delay(512);
    embassy_nrf::pac::RESET
        .network()
        .forceoff()
        .write(|w| w.set_forceoff(Forceoff::HOLD));
    cortex_m::asm::delay(128);
    embassy_nrf::pac::RESET
        .network()
        .forceoff()
        .write(|w| w.set_forceoff(Forceoff::RELEASE));
    unsafe {
        (0x50005618 as *mut u32).write_volatile(0);
    }
}
