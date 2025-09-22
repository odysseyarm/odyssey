#![no_std]
#![no_main]

const PROTOCOL_VERSION: u32 = 1;

#[macro_use]
mod utils;
mod image_mode;
mod imu;
mod init;
mod object_mode;
mod settings;
mod usb;

use core::cell::RefCell;

use embassy_executor::Spawner;
use embassy_nrf::{
    gpio::{Input, Level, Output, OutputDrive, Pull},
    nvmc::Nvmc,
    peripherals::{self, SPIM4},
    spim::{self, Spim},
    usb::vbus_detect::HardwareVbusDetect,
};
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::Delay;
use embassy_usb::class::cdc_acm;
use embedded_hal_bus::spi::ExclusiveDevice;
use imu::{Imu, ImuInterrupt};
use pag7661qn::{Pag7661Qn, Pag7661QnInterrupt, mode, spi::Pag7661QnSpi};
use settings::{Settings, init_settings};
use {defmt_rtt as _, panic_probe as _};

embassy_nrf::bind_interrupts!(struct Irqs {
    SERIAL1 => spim::InterruptHandler<peripherals::SERIAL1>;
    SPIM4 => spim::InterruptHandler<peripherals::SPIM4>;
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let (_core_peripherals, p) = init::init();
    defmt::info!("Initialized");

    static NVMC: static_cell::StaticCell<
        Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc>>,
    > = static_cell::StaticCell::new();
    let nvmc = NVMC.init_with(|| Mutex::new(RefCell::new(Nvmc::new(p.NVMC))));
    let nvmc = &*nvmc;

    // Reset the PAG
    let mut vis_resetn = Output::new(p.P1_06, Level::Low, OutputDrive::Standard);
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
    let spim = Spim::new(p.SPIM4, Irqs, p.P0_08, p.P0_10, p.P0_09, spim_config);
    let cs = Output::new(p.P0_11, Level::High, OutputDrive::Standard);
    let int_o = Input::new(p.P0_06, Pull::None);
    let Ok(spi_device) = ExclusiveDevice::new(spim, cs, Delay);
    let (mut pag, pag_int) =
        Pag7661Qn::init_spi(spi_device, embassy_time::Delay, int_o, mode::Idle)
            .await
            .unwrap();

    // let mut pag = pag.switch_mode(mode::Object).await.unwrap();
    // let mut pag_int = pag_int;
    // let mut objs = [pag7661qn::types::Object::DEFAULT; 16];
    // let objcnt = pag.wait_for_objects(&mut pag_int, &mut objs).await.unwrap();
    // defmt::info!("Got {=u8} objects", objcnt);
    // for (i, obj) in objs[..objcnt as usize].iter().enumerate() {
    //     defmt::info!("Object {=u8}:", i as u8);
    //     defmt::info!("    id = {}", obj.id());
    //     defmt::info!("    avg = {}", obj.avg());
    //     defmt::info!("    x = {}", obj.x());
    //     defmt::info!("    y = {}", obj.y());
    //     defmt::info!("    area = {}", obj.area());
    // }
    // let mut pag = pag.switch_mode(mode::Idle).await.unwrap();

    let (imu, imu_int) = imu::init(
        p.SERIAL1,
        p.P0_15.into(),
        p.P0_22.into(),
        p.P1_04.into(),
        p.P1_05.into(),
        p.P0_07.into(),
        p.P0_13.into(),
        p.TIMER0,
        p.PPI_CH0.into(),
        p.GPIOTE_CH0.into(),
    )
    .await;

    let (mut cdc_acm_class, usb) = usb::usb_device(p.USBD, nvmc);
    let nvmc = nvmc.borrow();

    let settings = init_settings(&nvmc, &mut pag).await;

    spawner.spawn(defmt::unwrap!(usb::run_usb(usb)));
    cdc_acm_class.wait_connection().await;
    defmt::info!("CDC-ACM connected");

    let (usb_snd, usb_rcv, usb_ctl) = cdc_acm_class.split_with_control();
    let ctx = CommonContext {
        usb_snd,
        usb_rcv,
        usb_ctl,
        pag: pag.as_dynamic_mode(),
        pag_int,
        imu,
        imu_int,
        settings,
        nvmc,
        img: image_mode::Context::take(),
        obj: object_mode::Context::take(),
    };

    match ctx.settings.transient.mode {
        protodongers::Mode::Image => {
            ctx.settings.transient.mode = protodongers::Mode::Object;
            ctx.settings.transient_write(nvmc);
            image_mode::image_mode(ctx).await;
        }
        protodongers::Mode::Object => {
            object_mode::object_mode(ctx).await;
        }
    }
}

type Pag<M> = Pag7661Qn<
    Pag7661QnSpi<ExclusiveDevice<Spim<'static, SPIM4>, Output<'static>, Delay>>,
    embassy_time::Delay,
    M,
>;
type PagInt = Pag7661QnInterrupt<Input<'static>>;
type UsbDriver =
    embassy_nrf::usb::Driver<'static, embassy_nrf::peripherals::USBD, HardwareVbusDetect>;

struct CommonContext {
    usb_snd: cdc_acm::Sender<'static, UsbDriver>,
    usb_rcv: cdc_acm::Receiver<'static, UsbDriver>,
    #[expect(dead_code)]
    usb_ctl: cdc_acm::ControlChanged<'static>,
    pag: Pag<mode::Mode>,
    pag_int: PagInt,
    imu: Imu,
    imu_int: ImuInterrupt,
    settings: &'static mut Settings,
    nvmc: &'static RefCell<Nvmc<'static>>,

    img: image_mode::Context,
    obj: object_mode::Context,
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
    unsafe { core::str::from_utf8_unchecked(buf) }
}
