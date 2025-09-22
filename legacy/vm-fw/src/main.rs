#![no_main]
#![no_std]
#![feature(never_type)]

#[macro_use]
mod utils;
mod pins;
mod usb;
mod settings;
mod imu;
mod fodtrigger;
mod object_mode;

use core::cell::RefCell;

use embassy_executor::Spawner;
use embassy_nrf::timer::Timer;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::{blocking_mutex::Mutex, mutex::Mutex as AsyncMutex};
use embassy_nrf::config::Config;
use embassy_nrf::gpio::{self, Output, OutputDrive};
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::{Peri, bind_interrupts, gpiote, peripherals, ppi};
use embassy_time::Delay;
use embassy_usb::class::cdc_acm;
use embedded_hal_bus::spi::ExclusiveDevice;
use paj7025::Paj7025;
use settings::{Settings, init_settings};
use crate::imu::{Imu, ImuInterrupt};
use crate::usb::UsbDriver;
use crate::utils::make_spi_dev;

use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    TWISPI0     => spim::InterruptHandler<peripherals::TWISPI0>;
    TWISPI1     => spim::InterruptHandler<peripherals::TWISPI1>;
    SPI2        => spim::InterruptHandler<peripherals::SPI2>;
    USBD        => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut config = Config::default();

    // If building for the VM context, bump GPIO voltage to 2.4V.
    #[cfg(context = "vm")]
    {
        config.dcdc.reg0_voltage = Some(embassy_nrf::config::Reg0Voltage::_2V4);
    }

    let p = embassy_nrf::init(config);
    let b = split_board!(p);
    defmt::info!("boot");
    
    static NVMC: static_cell::StaticCell<
        Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc>>,
    > = static_cell::StaticCell::new();
    let nvmc = NVMC.init_with(|| Mutex::new(RefCell::new(Nvmc::new(p.NVMC))));
    let nvmc = &*nvmc;

    // ========= PAG7025 (R2/R3) on TWISPI0 / TWISPI1 =========
    let mut cfg_paj = spim::Config::default();
    cfg_paj.frequency = spim::Frequency::M8;
    cfg_paj.mode = spim::MODE_3;
    cfg_paj.bit_order = spim::BitOrder::LSB_FIRST;

    let paj7025r2_spi = pins::Spi {
        sck: b.paj7025r2.sck.into(),
        miso: b.paj7025r2.miso.into(),
        mosi: b.paj7025r2.mosi.into(),
        cs: b.paj7025r2.cs.into(),
    };
    let paj7025r3_spi = pins::Spi {
        sck: b.paj7025r3.sck.into(),
        miso: b.paj7025r3.miso.into(),
        mosi: b.paj7025r3.mosi.into(),
        cs: b.paj7025r3.cs.into(),
    };
    let dev_r2 = make_spi_dev(p.TWISPI0, Irqs, paj7025r2_spi, cfg_paj.clone());
    let dev_r3 = make_spi_dev(p.TWISPI1, Irqs, paj7025r3_spi, cfg_paj);
    
    embassy_time::Timer::after_millis(1).await;

    let mut paj7025r2 = Paj7025::init(dev_r2).await.unwrap();
    let mut paj7025r3 = Paj7025::init(dev_r3).await.unwrap();

    let prod_id_r2 = paj7025r2
        .ll
        .control()
        .bank_0()
        .product_id()
        .read_async()
        .await
        .unwrap()
        .value();
    defmt::info!("PAJ7025 R2 Product ID: 0x{:x}", prod_id_r2);
    
    let prod_id_r3 = paj7025r3
        .ll
        .control()
        .bank_0()
        .product_id()
        .read_async()
        .await
        .unwrap()
        .value();
    defmt::info!("PAJ7025 R3 Product ID: 0x{:x}", prod_id_r3);
    
    paj7025r2.init_settings(false).await.unwrap();
    paj7025r3.init_settings(true).await.unwrap();

    let bmi270_spi = pins::Spi {
        sck: b.bmi270.sck.into(),
        miso: b.bmi270.miso.into(),
        mosi: b.bmi270.mosi.into(),
        cs: b.bmi270.cs.into(),
    };

    let (imu, imu_int) = imu::init::<_, _, 65535, 255>(
        p.SPI2,
        Irqs,
        bmi270_spi,
        b.bmi270.irq.into(),
    ).await.unwrap();

    let (mut cdc, usb) = usb::usb_device(p.USBD);
    spawner.spawn(defmt::unwrap!(usb::run_usb(usb)));

    let nvmc = nvmc.borrow();
    let settings = init_settings(nvmc, &mut paj7025r2, &mut paj7025r3).await.unwrap();

    cdc.wait_connection().await;
    defmt::info!("CDC-ACM connected");

    static PAJ7025R2_MUTEX: static_cell::StaticCell<AsyncMutex<NoopRawMutex, Paj>> = static_cell::StaticCell::new();
    static PAJ7025R3_MUTEX: static_cell::StaticCell<AsyncMutex<NoopRawMutex, Paj>> = static_cell::StaticCell::new();

    let paj7025r2 = PAJ7025R2_MUTEX.init(AsyncMutex::<NoopRawMutex, _>::new(paj7025r2));
    let paj7025r3 = PAJ7025R3_MUTEX.init(AsyncMutex::<NoopRawMutex, _>::new(paj7025r3));

    let (usb_snd, usb_rcv, usb_ctl) = cdc.split_with_control();
    let ctx = CommonContext {
        usb_snd,
        usb_rcv,
        usb_ctl,
        paj7025r2_group: (paj7025r2, b.paj7025r2.fod.into(), p.GPIOTE_CH0.into()),
        paj7025r3_group: (paj7025r3, b.paj7025r3.fod.into(), p.GPIOTE_CH1.into()),
        fod_set_ch: p.PPI_CH0.into(),
        fod_clr_ch: p.PPI_CH1.into(),
        imu,
        imu_int,
        settings,
        nvmc,
        obj: object_mode::Context::take(),
    };

    let _ = object_mode::object_mode(ctx, p.TIMER1).await;
}

type Paj = Paj7025<ExclusiveDevice<Spim<'static>, Output<'static>, Delay>, embedded_hal_bus::spi::DeviceError<spim::Error, core::convert::Infallible>>;
type PajGroup<'a> = (&'a AsyncMutex<NoopRawMutex, Paj>, Peri<'a, gpio::AnyPin>, Peri<'a, gpiote::AnyChannel>);

struct CommonContext<const IMU_N: usize> {
    usb_snd: cdc_acm::Sender<'static, UsbDriver>,
    usb_rcv: cdc_acm::Receiver<'static, UsbDriver>,
    #[expect(dead_code)]
    usb_ctl: cdc_acm::ControlChanged<'static>,
    paj7025r2_group: PajGroup<'static>,
    paj7025r3_group: PajGroup<'static>,
    fod_set_ch: Peri<'static, ppi::AnyConfigurableChannel>,
    fod_clr_ch: Peri<'static, ppi::AnyConfigurableChannel>,
    imu: Imu<IMU_N>,
    imu_int: ImuInterrupt,
    settings: &'static mut Settings,
    nvmc: &'static RefCell<Nvmc<'static>>,
    obj: object_mode::Context,
}
