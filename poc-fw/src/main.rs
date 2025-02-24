#![no_std]
#![no_main]

const PROTOCOL_VERSION: u32 = 1;

mod usb;

use core::str;

use embassy_executor::Spawner;
use embassy_nrf::{
    Peripherals,
    gpio::{Input, Level, Output, OutputDrive, Pull},
    pac,
    peripherals::{self, SPIM4},
    spim::{self, Spim},
};
use embassy_sync::{
    blocking_mutex::raw::NoopRawMutex,
    channel::{Channel, Receiver, Sender},
};
use embedded_hal_bus::spi::{ExclusiveDevice, NoDelay};
use pag7661qn::{Pag7661Qn, mode, spi::Pag7661QnSpi};
use static_cell::ConstStaticCell;
use usb::{wait_for_serial, write_serial};
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
        let hwid_b0 = Input::new(&mut peripherals.P1_12, Pull::None);
        let hwid_b1 = Input::new(&mut peripherals.P1_11, Pull::None);
        let hwid = ((hwid_b1.is_high() as u8) << 1) | (hwid_b0.is_high() as u8);
        defmt::info!("HWID = {=u8}", hwid);
    }

    (core_peripherals, peripherals)
}

embassy_nrf::bind_interrupts!(struct Irqs {
    SPIM4 => spim::InterruptHandler<peripherals::SPIM4>;
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
});

const NUM_BUFFERS: usize = 2;
static SHARED_BUFFERS: ConstStaticCell<[[u8; 320 * 240]; NUM_BUFFERS]> =
    ConstStaticCell::new([[0; 320 * 240]; NUM_BUFFERS]);
type ImageBufferReceiver<const N: usize = NUM_BUFFERS> =
    Receiver<'static, NoopRawMutex, &'static mut [u8; 320 * 240], N>;
type ImageBufferSender<const N: usize = NUM_BUFFERS> =
    Sender<'static, NoopRawMutex, &'static mut [u8; 320 * 240], N>;
type ImageBufferChannel<const N: usize = NUM_BUFFERS> =
    Channel<NoopRawMutex, &'static mut [u8; 320 * 240], N>;
static FREE_BUFFERS: ConstStaticCell<ImageBufferChannel<2>> =
    ConstStaticCell::new(ImageBufferChannel::new());
static IMAGE_BUFFERS: ConstStaticCell<ImageBufferChannel<1>> =
    ConstStaticCell::new(ImageBufferChannel::new());

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let (_core_peripherals, peripherals) = init();

    let (ref mut cdc_acm_class, usb) = usb::usb_device(peripherals.USBD);
    spawner.must_spawn(usb::run_usb(usb));

    // Reset the PAG
    let mut vis_resetn = Output::new(peripherals.P1_06, Level::Low, OutputDrive::Standard);
    // the datasheet doesn't say how long the reset pin needs to be asserted
    embassy_time::Timer::after_micros(500).await;
    vis_resetn.set_high();

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
    let int_o = Input::new(peripherals.P0_06, Pull::None);
    let Ok(spi_device) = ExclusiveDevice::new_no_delay(spim, cs);
    let mut pag = Pag7661Qn::init_spi(spi_device, embassy_time::Delay, int_o, mode::Idle)
        .await
        .unwrap();

    pag.set_sensor_fps(30).await.unwrap();
    pag.set_sensor_exposure_us(true, 12700).await.unwrap();
    pag.set_sensor_gain(1).await.unwrap();
    let pag = pag.switch_mode(mode::Image).await.unwrap();

    cdc_acm_class.wait_connection().await;
    defmt::info!("CDC-ACM connected");

    let [b0, b1] = SHARED_BUFFERS.take();
    let free_buffers = FREE_BUFFERS.take();
    let image_buffers = IMAGE_BUFFERS.take();
    free_buffers.try_send(b0).unwrap();
    free_buffers.try_send(b1).unwrap();
    spawner.must_spawn(image_loop(
        pag,
        free_buffers.receiver(),
        image_buffers.sender(),
    ));

    loop {
        match wait_for_serial(cdc_acm_class).await {
            b'a' => {
                let image = image_buffers.receive().await;
                write_serial(cdc_acm_class, image).await;
                free_buffers.send(image).await;
            }

            b'i' => {
                write_serial(cdc_acm_class, &PROTOCOL_VERSION.to_le_bytes()).await;
                write_serial(cdc_acm_class, &device_id()[..6]).await;
                // sensor resolution
                let [a, b] = 320u16.to_le_bytes(); // width
                let [c, d] = 240u16.to_le_bytes(); // height

                // object resolution
                let [e, f, g, h] = (320u32 * (1 << 6)).to_le_bytes();
                let [i, j, k, l] = (240u32 * (1 << 6)).to_le_bytes();

                let buf = [a, b, c, d, e, f, g, h, i, j, k, l];
                write_serial(cdc_acm_class, &buf).await;
            }

            cmd => defmt::error!("Unknown command '{=u8}'", cmd),
        }
    }
}

#[embassy_executor::task]
async fn image_loop(
    mut pag: Pag7661Qn<
        Pag7661QnSpi<ExclusiveDevice<Spim<'static, SPIM4>, Output<'static>, NoDelay>>,
        embassy_time::Delay,
        Input<'static>,
        mode::Image,
    >,
    free_buffers: ImageBufferReceiver<2>,
    image_buffers: ImageBufferSender<1>,
) {
    loop {
        let buf = free_buffers.receive().await;
        pag.get_frame(buf).await.unwrap();
        // the image is mirrored for some reason
        buf.chunks_mut(320).for_each(|l| l.reverse());
        image_buffers.send(buf).await;
    }
}

fn device_id() -> [u8; 8] {
    let ficr = embassy_nrf::pac::FICR;
    let low = ficr.info().deviceid(0).read();
    let high = ficr.info().deviceid(1).read();
    let [a, b, c, d] = low.to_le_bytes();
    let [e, f, g, h] = high.to_le_bytes();
    [a, b, c, d, e, f, g, h]
}

fn device_id_str(buf: &mut [u8; 16]) -> &str {
    const CHARACTERS: [u8; 16] = *b"0123456789ABCDEF";
    let id = device_id();
    for (a, b) in id.into_iter().zip(buf.chunks_mut(2)) {
        b[0] = CHARACTERS[(a >> 4) as usize];
        b[1] = CHARACTERS[(a % 16) as usize];
    }
    unsafe { str::from_utf8_unchecked(buf) }
}
