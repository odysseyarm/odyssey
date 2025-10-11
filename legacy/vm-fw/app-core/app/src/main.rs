#![no_main]
#![no_std]
#![feature(never_type)]

#[macro_use]
mod utils;
mod fodtrigger;
mod imu;
mod object_mode;
mod pins;
mod settings;
mod usb;

#[cfg(context = "atslite1")]
mod nrf5340_init;

use core::cell::RefCell;
use core::marker::PhantomData;

use embassy_executor::Spawner;
#[cfg(context = "vm")]
use embassy_nrf::config::Config;
use embassy_nrf::gpio::{self, Output};
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::{Peri, bind_interrupts, gpiote, interrupt, peripherals, ppi, timer};
use embassy_nrf::usb::{Endpoint, In, Out};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::Delay;
use embassy_usb::driver::Endpoint as _;
use embedded_hal_bus::spi::ExclusiveDevice;
use paj7025::Paj7025;
use settings::{Settings, init_settings};
use {defmt_rtt as _, panic_probe as _};

use crate::fodtrigger::FOD_TICK_SIG;
#[cfg(context = "vm")]
use crate::imu::bmi::{Imu, ImuInterrupt};
#[cfg(context = "atslite1")]
use crate::imu::icm::{Imu, ImuInterrupt};
use crate::utils::make_spi_dev;

#[cfg(context = "vm")]
bind_interrupts!(struct Irqs {
    TWISPI0     => spim::InterruptHandler<peripherals::TWISPI0>;
    TWISPI1     => spim::InterruptHandler<peripherals::TWISPI1>;
    SPI2        => spim::InterruptHandler<peripherals::SPI2>;
    USBD        => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler;
    TIMER1      => TIrq<peripherals::TIMER1>;
});

#[cfg(context = "atslite1")]
bind_interrupts!(struct Irqs {
    SERIAL0 => spim::InterruptHandler<peripherals::SERIAL0>;
    SERIAL1 => spim::InterruptHandler<peripherals::SERIAL1>;
    SPIM4 => spim::InterruptHandler<peripherals::SPIM4>;
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
    TIMER1      => TIrq<peripherals::TIMER1>;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    #[cfg(context = "vm")]
    let mut config = Config::default();

    // If building for the VM context, bump GPIO voltage to 2.4V.
    #[cfg(context = "vm")]
    {
        config.dcdc.reg0_voltage = Some(embassy_nrf::config::Reg0Voltage::_2V4);
    }

    #[cfg(context = "atslite1")]
    let (_core_peripherals, p) = nrf5340_init::init();

    #[cfg(context = "vm")]
    let p = embassy_nrf::init(config);
    let b = split_board!(p);
    defmt::info!("boot");

    static NVMC: static_cell::StaticCell<Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, RefCell<Nvmc>>> =
        static_cell::StaticCell::new();
    let nvmc = NVMC.init_with(|| Mutex::new(RefCell::new(Nvmc::new(p.NVMC))));
    let nvmc = &*nvmc;

    // ========= PAG7025 (R2/R3) =========
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

    #[cfg(context = "vm")]
    let dev_r2 = make_spi_dev(p.TWISPI0, Irqs, paj7025r2_spi, cfg_paj.clone());
    #[cfg(context = "vm")]
    let dev_r3 = make_spi_dev(p.TWISPI1, Irqs, paj7025r3_spi, cfg_paj);

    #[cfg(context = "atslite1")]
    let dev_r2 = make_spi_dev(p.SERIAL0, Irqs, paj7025r2_spi, cfg_paj.clone());
    #[cfg(context = "atslite1")]
    let dev_r3 = make_spi_dev(p.SERIAL1, Irqs, paj7025r3_spi, cfg_paj);

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

    paj7025r2.init_settings(true, 0x10, 0x00).await.unwrap();
    paj7025r3.init_settings(true, 0x10, 0x02).await.unwrap();

    #[cfg(context = "vm")]
    let bmi270_spi = pins::Spi {
        sck: b.bmi270.sck.into(),
        miso: b.bmi270.miso.into(),
        mosi: b.bmi270.mosi.into(),
        cs: b.bmi270.cs.into(),
    };
    
    #[cfg(context = "vm")]
    let (imu, imu_int) = imu::bmi::init::<_, _, 65535, 255>(p.SPI2, Irqs, bmi270_spi, b.bmi270.irq.into())
        .await
        .unwrap();

    #[cfg(context = "atslite1")]
    // Erratum 135
    unsafe {
        (0x5000ac04 as *mut u32).write_volatile(1);
    }

    #[cfg(context = "atslite1")]
    let (imu, imu_int) = imu::icm::init(
        p.SPIM4,
        b.icm42688v.cs.into(),
        b.icm42688v.sck.into(),
        b.icm42688v.miso.into(),
        b.icm42688v.mosi.into(),
        b.icm42688v.int1.into(),
        b.icm42688v.clkin.into(),
        p.TIMER0,
        p.PPI_CH0.into(),
        p.GPIOTE_CH0.into(),
    )
    .await;

    let (usb, mut usb_snd, mut usb_rcv) = usb::usb_device(p.USBD);
    let _ = spawner.spawn(usb::run_usb(usb));

    let nvmc = nvmc.borrow();
    let settings = init_settings(nvmc, &mut paj7025r2, &mut paj7025r3).await.unwrap();

    usb_snd.wait_enabled().await;
    usb_rcv.wait_enabled().await;

    defmt::info!("USB Endpoints enabled");

    static PAJ7025R2_MUTEX: static_cell::StaticCell<AsyncMutex<NoopRawMutex, Paj>> = static_cell::StaticCell::new();
    static PAJ7025R3_MUTEX: static_cell::StaticCell<AsyncMutex<NoopRawMutex, Paj>> = static_cell::StaticCell::new();

    let paj7025r2 = PAJ7025R2_MUTEX.init(AsyncMutex::<NoopRawMutex, _>::new(paj7025r2));
    let paj7025r3 = PAJ7025R3_MUTEX.init(AsyncMutex::<NoopRawMutex, _>::new(paj7025r3));

    let ctx = CommonContext {
        usb_snd,
        usb_rcv,
        paj7025r2_group: (paj7025r2, b.paj7025r2.fod.into(), p.GPIOTE_CH1.into()),
        paj7025r3_group: (paj7025r3, b.paj7025r3.fod.into(), p.GPIOTE_CH2.into()),
        fod_set_ch: p.PPI_CH1.into(),
        fod_clr_ch: p.PPI_CH2.into(),
        imu,
        imu_int,
        settings,
        nvmc,
    };

    let _ = object_mode::object_mode(ctx, p.TIMER1).await;
}

type Paj = Paj7025<
    ExclusiveDevice<Spim<'static>, Output<'static>, Delay>,
    embedded_hal_bus::spi::DeviceError<spim::Error, core::convert::Infallible>,
>;
type PajGroup<'a> = (
    &'a AsyncMutex<NoopRawMutex, Paj>,
    Peri<'a, gpio::AnyPin>,
    Peri<'a, gpiote::AnyChannel>,
);

#[cfg(context = "vm")]
struct CommonContext<const IMU_N: usize> {
    usb_snd: Endpoint<'static, In>,
    usb_rcv: Endpoint<'static, Out>,
    paj7025r2_group: PajGroup<'static>,
    paj7025r3_group: PajGroup<'static>,
    fod_set_ch: Peri<'static, ppi::AnyConfigurableChannel>,
    fod_clr_ch: Peri<'static, ppi::AnyConfigurableChannel>,
    imu: Imu<IMU_N>,
    imu_int: ImuInterrupt,
    settings: &'static mut Settings,
    nvmc: &'static RefCell<Nvmc<'static>>,
}

#[cfg(context = "atslite1")]
struct CommonContext {
    usb_snd: Endpoint<'static, In>,
    usb_rcv: Endpoint<'static, Out>,
    paj7025r2_group: PajGroup<'static>,
    paj7025r3_group: PajGroup<'static>,
    fod_set_ch: Peri<'static, ppi::AnyConfigurableChannel>,
    fod_clr_ch: Peri<'static, ppi::AnyConfigurableChannel>,
    imu: Imu,
    imu_int: ImuInterrupt,
    settings: &'static mut Settings,
    nvmc: &'static RefCell<Nvmc<'static>>,
}

pub struct TIrq<T: timer::Instance> {
    _phantom: PhantomData<T>,
}

impl<T: timer::Instance> interrupt::typelevel::Handler<T::Interrupt> for TIrq<T> {
    unsafe fn on_interrupt() {
        let regs = unsafe { embassy_nrf::pac::timer::Timer::from_ptr(embassy_nrf::pac::TIMER1.as_ptr()) };
        
        regs.events_compare(1).write(|w| *w = 0);
        FOD_TICK_SIG.signal(());
    }
}
