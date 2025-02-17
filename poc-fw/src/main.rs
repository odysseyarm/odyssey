#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_nrf::{
    gpio::{self, Input, Level, Output, OutputDrive, Pull},
    pac, peripherals,
    spim::{self, Spim},
    Peripheral, Peripherals,
};
use embedded_hal_bus::spi::ExclusiveDevice;
use pag7661qn::Pag7661Qn;
use static_cell::ConstStaticCell;
use {defmt_rtt as _, panic_probe as _};

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

fn init() -> (cortex_m::Peripherals, Peripherals) {
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
        let hwid_b0 = Input::new((&mut peripherals.P1_12).into_ref(), Pull::None);
        let hwid_b1 = Input::new((&mut peripherals.P1_11).into_ref(), Pull::None);
        let hwid = ((hwid_b1.is_high() as u8) << 1) | (hwid_b0.is_high() as u8);
        defmt::info!("HWID = {=u8}", hwid);
    }

    (core_peripherals, peripherals)
}

embassy_nrf::bind_interrupts!(struct Irqs {
    SPIM4 => spim::InterruptHandler<peripherals::SPIM4>;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let (_core_peripherals, peripherals) = init();

    // Erratum 135
    unsafe {
        (0x5000ac04 as *mut u32).write_volatile(1);
    }
    let mut spim_config = spim::Config::default();
    spim_config.frequency = spim::Frequency::M16;
    spim_config.mode = spim::MODE_3;
    spim_config.bit_order = spim::BitOrder::MSB_FIRST;
    spim_config.orc = 0;
    spim_config.sck_drive = OutputDrive::HighDrive;
    spim_config.mosi_drive = OutputDrive::HighDrive;
    let spim = Spim::new(
        peripherals.SPIM4,
        Irqs,
        peripherals.P0_08,
        peripherals.P0_10,
        peripherals.P0_09,
        spim_config,
    );
    let cs = Output::new(peripherals.P0_11, Level::High, OutputDrive::Standard);
    let int_o = Input::new(peripherals.P0_06, Pull::Down);
    let Ok(spi_device) = ExclusiveDevice::new_no_delay(spim, cs);
    let mut pag = Pag7661Qn::new_spi(
        spi_device,
        embassy_time::Delay,
        int_o,
        pag7661qn::mode::Idle,
    )
    .await
    .unwrap();

    defmt::info!("fps = {=u8}", pag.get_sensor_fps().await.unwrap());
    pag.set_sensor_fps(120).await.unwrap();
    defmt::info!("fps = {=u8}", pag.get_sensor_fps().await.unwrap());
    let mut pag = pag.switch_mode(pag7661qn::mode::Image).await.unwrap();

    defmt::info!("wait for frame");
    static IMAGE: ConstStaticCell<[u8; 320 * 240]> = ConstStaticCell::new([0; 320 * 240]);
    let image = IMAGE.take();
    loop {
        pag.get_frame(image).await.unwrap();
        // let min = image.iter().min();
        // let max = image.iter().max();
        // let avg = image.iter().map(|&v| v as u32).sum::<u32>() / (320*240);
        defmt::info!("got frame: {=[u8]}", image[..32]);
        // defmt::info!("min: {}, max: {}, avg: {}", min, max, avg);
    }
}
