use embassy_nrf::{
    Peripherals,
    gpio::{Input, Pull},
    pac,
};

fn start_network_core(delay: &mut cortex_m::delay::Delay) {
    use embassy_nrf::pac::{self, reset::vals::Forceoff};
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

fn configure_internal_capacitors() {
    use pac::oscillators::vals::Intcap;
    // HFXO
    const CAPACITANCE: f32 = 15.5; // pF
    let mut slope = pac::FICR.xosc32mtrim().read().slope() as i32;
    let offset = pac::FICR.xosc32mtrim().read().offset() as i32;
    if slope >= 16 {
        slope -= 32;
    }
    defmt::trace!("XOSC32MTRIM.SLOPE = {=i32}", slope);
    defmt::trace!("XOSC32MTRIM.OFFSET = {=i32}", offset);
    let m = const { (CAPACITANCE * 2.0) as i32 - 14 };
    let capvalue = (((slope + 56) * m) + ((offset - 8) << 4) + 32) >> 6;
    defmt::trace!("XOSC32MCAPS.CAPVALUE = {=i32}", capvalue);
    pac::OSCILLATORS.xosc32mcaps().write(|w| {
        w.set_capvalue(capvalue as u8);
        w.set_enable(true);
    });

    // LFXO
    // 7 pF
    pac::OSCILLATORS
        .xosc32ki()
        .intcap()
        .write(|w| w.set_intcap(Intcap::C7PF));
}

pub fn init() -> (cortex_m::Peripherals, Peripherals) {
    use embassy_nrf::{
        config::{Config, HfclkSource, LfclkSource},
        pac::uicr::vals::Vreghvout,
    };

    let mut needs_reset = false;
    // Configure VDD to 3.3V
    let uicr = pac::UICR;
    let v = uicr.vreghvout().read().vreghvout();
    defmt::info!("VREGHVOUT = {}", v);
    if v != Vreghvout::_3V3 {
        defmt::info!("Changing to _3V3");
        let nvmc = pac::NVMC;
        nvmc.config()
            .write(|w| w.set_wen(pac::nvmc::vals::Wen::WEN));
        while !nvmc.ready().read().ready() {}
        uicr.vreghvout().write(|v| v.set_vreghvout(Vreghvout::_3V3));
        while !nvmc.ready().read().ready() {}
        nvmc.config()
            .write(|w| w.set_wen(pac::nvmc::vals::Wen::REN));
        while !nvmc.ready().read().ready() {}
        needs_reset = true;
    }

    let mut config = Config::default();
    // The BC40C has the hardware to support this, but I don't want to think about erratum 160
    // Hopefully erratum 166 doesn't show up, the workaround increases power usage ;-;
    config.dcdc.regh = true;
    // config.dcdc.regmain = true;
    // config.dcdc.regradio = true;
    config.hfclk_source = HfclkSource::ExternalXtal;
    config.lfclk_source = LfclkSource::ExternalXtal;

    configure_internal_capacitors();
    // Enable instruction cache
    pac::CACHE.enable().write(|w| w.set_enable(true));
    // Set clock to 128 MHz
    pac::CLOCK
        .hfclkctrl()
        .write(|w| w.set_hclk(pac::clock::vals::Hclk::DIV1));

    let mut peripherals = embassy_nrf::init(config);
    if needs_reset {
        defmt::info!("Resetting...");
        cortex_m::peripheral::SCB::sys_reset();
    }

    // probe-rs complains if network core is locked or off
    // https://github.com/probe-rs/probe-rs/issues/3053
    let mut core_peripherals = cortex_m::Peripherals::take().unwrap();
    let mut systick_delay = cortex_m::delay::Delay::new(core_peripherals.SYST, 64_000_000);
    start_network_core(&mut systick_delay);
    core_peripherals.SYST = systick_delay.free();

    {
        let hwid_b0 = Input::new(peripherals.P1_12.reborrow(), Pull::None);
        let hwid_b1 = Input::new(peripherals.P1_11.reborrow(), Pull::None);
        let hwid = ((hwid_b1.is_high() as u8) << 1) | (hwid_b0.is_high() as u8);
        defmt::info!("HWID = {=u8}", hwid);
    }

    (core_peripherals, peripherals)
}
