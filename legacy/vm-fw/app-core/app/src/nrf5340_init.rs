use embassy_nrf::config::{Config, HfclkSource, LfclkSource, LfxoCapacitance};
use embassy_nrf::{Peripherals, pac};

/// Configure SPU so the network core can access the ICMSG {RX,TX} regions.
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
    // Engineering A revision has 32 regions of 32KB, later revisions have 64 regions of 16KB
    // Configure for both to be safe
    const NUM_FLASH_REGIONS: usize = 64; // Maximum number of flash regions

    #[inline]
    fn to_ram_region_index(addr: u32) -> u32 {
        (addr.saturating_sub(RAM_BASE)) / RAM_REGION_SIZE
    }

    let spu = embassy_nrf::pac::SPU_S;

    // Configure all flash regions to be non-secure and accessible by network core
    for i in 0..NUM_FLASH_REGIONS {
        spu.flashregion(i).perm().write(|w| {
            w.set_read(true);
            w.set_write(true);
            w.set_execute(true);
            w.set_secattr(false); // Non-secure
        });
    }

    // Configure RAM regions for ICMSG
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

    // Configure EXTDOMAIN[0] for network core access
    spu.extdomain(0).perm().write(|w| {
        w.set_securemapping(vals::ExtdomainPermSecuremapping::NON_SECURE);
    });
}

fn start_network_core(delay: &mut cortex_m::delay::Delay) {
    use embassy_nrf::pac::reset::vals::Forceoff;
    use embassy_nrf::pac::{self};
    // Erratum 161
    unsafe {
        (0x50005618 as *mut u32).write_volatile(1);
    }
    pac::RESET
        .network()
        .forceoff()
        .write(|w| w.set_forceoff(Forceoff::RELEASE));
    delay.delay_us(5);
    pac::RESET
        .network()
        .forceoff()
        .write(|w| w.set_forceoff(Forceoff::HOLD));
    delay.delay_us(1);
    pac::RESET
        .network()
        .forceoff()
        .write(|w| w.set_forceoff(Forceoff::RELEASE));
    unsafe {
        (0x50005618 as *mut u32).write_volatile(0);
    }
}

pub fn init() -> (cortex_m::Peripherals, Peripherals) {
    let mut config = Config::default();

    config.dcdc.regh = true;
    // Erratum 160 workaround is not implemented
    config.dcdc.regmain = false;
    config.dcdc.regradio = false;
    config.hfclk_source = HfclkSource::ExternalXtal; // HFXO is required for radio
    config.lfclk_source = LfclkSource::ExternalXtal;

    // Adjust as needed
    config.internal_capacitors.hfxo = None;
    config.internal_capacitors.lfxo = Some(LfxoCapacitance::_7pF);

    // Configure SPU BEFORE any other initialization to allow network core access
    unsafe {
        configure_spu_for_network_core();
    }

    // Enable instruction cache
    pac::CACHE.enable().write(|w| w.set_enable(true));
    // Set clock to 128 MHz (avoid erratum 166)
    pac::CLOCK
        .hfclkctrl()
        .write(|w| w.set_hclk(pac::clock::vals::Hclk::DIV1));

    let peripherals = embassy_nrf::init(config);

    // probe-rs complains if network core is locked or off
    // https://github.com/probe-rs/probe-rs/issues/3053
    let mut core_peripherals = cortex_m::Peripherals::take().unwrap();
    let mut systick_delay = cortex_m::delay::Delay::new(core_peripherals.SYST, 128_000_000);

    start_network_core(&mut systick_delay);
    core_peripherals.SYST = systick_delay.free();
    (core_peripherals, peripherals)
}
