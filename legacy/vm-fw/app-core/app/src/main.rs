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
mod battery_model;
#[cfg(context = "atslite1")]
mod ble;
#[cfg(context = "atslite1")]
mod nrf5340_init;
#[cfg(context = "atslite1")]
mod pmic_leds;
#[cfg(context = "atslite1")]
mod power;
#[cfg(context = "atslite1")]
mod power_button;
#[cfg(context = "atslite1")]
mod power_state;

use core::marker::PhantomData;

use embassy_executor::Spawner;
#[cfg(context = "vm")]
use embassy_nrf::config::Config;
#[cfg(context = "atslite1")]
use embassy_nrf::gpio::Input;
use embassy_nrf::gpio::{self, Output, Pin as _};
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::spim::{self, Spim};
#[cfg(context = "atslite1")]
use embassy_nrf::twim::{self, Twim};
use embassy_nrf::usb::{Endpoint, In, Out};
use embassy_nrf::{Peri, bind_interrupts, gpiote, interrupt, peripherals, ppi, timer};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::Delay;
use embassy_usb::driver::Endpoint as _;
use embedded_hal_bus::spi::ExclusiveDevice;
#[cfg(context = "atslite1")]
use npm1300_rs::NPM1300;
use paj7025::Paj7025;
use settings::{Settings, init_settings};
#[cfg(context = "atslite1")]
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

use crate::fodtrigger::FOD_TICK_SIG;
#[cfg(context = "vm")]
use crate::imu::bmi::{Imu, ImuInterrupt};
#[cfg(context = "atslite1")]
use crate::imu::icm::{Imu, ImuInterrupt};
#[cfg(context = "atslite1")]
use crate::pmic_leds::{LedState, PmicLedAnimator};
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
    SERIAL2 => twim::InterruptHandler<peripherals::SERIAL2>;
    SPIM4 => spim::InterruptHandler<peripherals::SPIM4>;
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
    TIMER1      => TIrq<peripherals::TIMER1>;
    IPC => embassy_nrf::ipc::InterruptHandler<peripherals::IPC>;
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

    #[cfg(context = "atslite1")]
    let twim = Twim::new(
        p.SERIAL2,
        Irqs,
        b.pmic.sda,
        b.pmic.scl,
        twim::Config::default(),
        &mut [],
    );
    #[cfg(context = "atslite1")]
    static PMIC: StaticCell<embassy_sync::mutex::Mutex<NoopRawMutex, NPM1300<Twim, Delay>>> = StaticCell::new();

    #[cfg(context = "atslite1")]
    let mut pmic = NPM1300::new(twim, Delay);

    #[cfg(context = "atslite1")]
    power::pmic_setup(&mut pmic).await.unwrap();

    #[cfg(context = "atslite1")]
    power::configure_and_start_charging(&mut pmic, npm1300_rs::sysreg::VbusInCurrentLimit::MA100)
        .await
        .unwrap();

    #[cfg(context = "atslite1")]
    let pmic = PMIC.init(embassy_sync::mutex::Mutex::new(pmic));

    spawner.must_spawn(pmic_irq_task(b.pmic.irq.into(), pmic));

    #[cfg(context = "atslite1")]
    let handle = {
        use crate::pmic_leds::LedIdx;

        let (anim, handle) = PmicLedAnimator::new(pmic, LedIdx::Ch0, LedIdx::Ch1, LedIdx::Ch2)
            .await
            .unwrap();

        spawner.must_spawn(led_task(anim));

        handle
    };

    let mut pwr_btn = Input::new(b.pmic.pwr_btn, gpio::Pull::Up);

    handle_boot_vbus_policy(pmic, &handle).await.unwrap();
    handle_pending_ship(pmic, &handle, &mut pwr_btn).await.unwrap();

    #[cfg(context = "atslite1")]
    handle.set_state(LedState::TurningOn).await;

    #[cfg(context = "atslite1")]
    static PMIC_LEDS_HANDLE: StaticCell<pmic_leds::PmicLedsHandle> = StaticCell::new();
    #[cfg(context = "atslite1")]
    let handle = PMIC_LEDS_HANDLE.init(handle);

    #[cfg(context = "atslite1")]
    spawner.must_spawn(power_btn_task(pwr_btn, pmic, handle));

    #[cfg(context = "atslite1")]
    spawner.must_spawn(power_state::power_state_task(pmic, handle));

    let nvmc = embassy_embedded_hal::adapter::BlockingAsync::new(Nvmc::new(p.NVMC));

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

    let (usb, mut usb_snd, mut usb_rcv, configured_sig) = usb::usb_device(p.USBD);
    let _ = spawner.spawn(usb::run_usb(usb));

    let settings = init_settings(&spawner, nvmc, &mut paj7025r2, &mut paj7025r3)
        .await
        .unwrap();

    if configured_sig.wait().await {
        pmic.lock()
            .await
            .set_vbus_in_current_limit(npm1300_rs::sysreg::VbusInCurrentLimit::MA500)
            .await
            .unwrap();
    }

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

#[cfg(context = "atslite1")]
#[embassy_executor::task]
async fn pmic_irq_task(
    irq_pin: Peri<'static, gpio::AnyPin>,
    npm1300: &'static embassy_sync::mutex::Mutex<NoopRawMutex, NPM1300<Twim<'static>, Delay>>,
) {
    let port = irq_pin.port();
    let pin = irq_pin.pin();

    crate::utils::set_pin_sense(&port, pin, embassy_nrf::pac::gpio::vals::Sense::HIGH);

    let mut irq = gpio::Input::new(irq_pin, gpio::Pull::None);

    loop {
        irq.wait_for_high().await;
        if !npm1300
            .lock()
            .await
            .get_vbus_in_status()
            .await
            .unwrap()
            .is_vbus_in_present
        {
            npm1300
                .lock()
                .await
                .clear_vbusin0_event_mask(npm1300_rs::mainreg::Vbusin0EventMask::VBUS_REMOVED)
                .await
                .unwrap();
            defmt::debug!("VBUS removed");
            crate::power::notify_vbus_removed();
        }
        npm1300
            .lock()
            .await
            .clear_vbusin1_event_mask(
                npm1300_rs::mainreg::Vbusin1EventMask::CC1_STATE_CHANGE
                    | npm1300_rs::mainreg::Vbusin1EventMask::CC2_STATE_CHANGE,
            )
            .await
            .unwrap();
        let cc_status = npm1300.lock().await.get_vbus_cc_status().await.unwrap();
        use npm1300_rs::sysreg::VbusInCcCmp;
        if {
            (match cc_status.vbusin_cc1_status {
                VbusInCcCmp::MA1500HighPower | VbusInCcCmp::MA3000HighPower => true,
                _ => {
                    defmt::trace!("CC1 is default-usb or disconnected");
                    false
                }
            }) || match cc_status.vbusin_cc2_status {
                VbusInCcCmp::MA1500HighPower | VbusInCcCmp::MA3000HighPower => true,
                _ => {
                    defmt::trace!("CC2 is default-usb or disconnected");
                    false
                }
            }
        } {
            defmt::debug!("High-power CC detected -> 500mA limit");
            npm1300
                .lock()
                .await
                .set_vbus_in_current_limit(npm1300_rs::sysreg::VbusInCurrentLimit::MA500)
                .await
                .unwrap();
        }
    }
}

#[cfg(context = "atslite1")]
#[embassy_executor::task]
async fn led_task(mut anim: PmicLedAnimator<'static, Twim<'static>, Delay>) -> ! {
    anim.run().await
}

#[embassy_executor::task]
async fn power_btn_task(
    btn: Input<'static>,
    pmic: &'static embassy_sync::mutex::Mutex<
        embassy_sync::blocking_mutex::raw::NoopRawMutex,
        npm1300_rs::NPM1300<Twim<'static>, Delay>,
    >,
    leds: &'static crate::pmic_leds::PmicLedsHandle,
) {
    power_button::power_button_loop(btn, pmic, leds).await;
}

#[cfg(context = "atslite1")]
async fn handle_pending_ship<I2c, Delay>(
    pmic: &embassy_sync::mutex::Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, npm1300_rs::NPM1300<I2c, Delay>>,
    leds: &pmic_leds::PmicLedsHandle,
    pwr_btn: &mut Input<'_>,
) -> Result<(), npm1300_rs::NPM1300Error<I2c::Error>>
where
    I2c: embedded_hal_async::i2c::I2c,
    Delay: embedded_hal_async::delay::DelayNs,
{
    let pending_ship = crate::power::take_pending_ship_flag();

    if pending_ship {
        let vbus_present = pmic.lock().await.get_vbus_in_status().await?.is_vbus_in_present;
        if !vbus_present {
            defmt::trace!("Pending-ship and VBUS gone -> enter ship now");
            leds.set_state(LedState::Off).await;
            pmic.lock().await.enter_ship_mode().await?;
        } else {
            use embassy_time::Duration;

            defmt::trace!("Pending-ship but VBUS present -> wait for VBUS removal");
            embassy_futures::select::select(
                async {
                    power::VBUS_REMOVED_SIG.wait().await;
                    pmic.lock().await.enter_ship_mode().await.unwrap();
                    loop {}
                },
                power_button::wait_for_power_button_full_press(pwr_btn, Duration::from_millis(96)),
            )
            .await;
        }
    }

    Ok(())
}

#[cfg(context = "atslite1")]
async fn handle_boot_vbus_policy<I2c, Delay>(
    pmic: &embassy_sync::mutex::Mutex<embassy_sync::blocking_mutex::raw::NoopRawMutex, npm1300_rs::NPM1300<I2c, Delay>>,
    leds: &pmic_leds::PmicLedsHandle,
) -> Result<(), npm1300_rs::NPM1300Error<I2c::Error>>
where
    I2c: embedded_hal_async::i2c::I2c,
    Delay: embedded_hal_async::delay::DelayNs,
{
    use embassy_nrf::reset::ResetReason;

    let rr = embassy_nrf::reset::read_reasons();
    embassy_nrf::reset::clear_reasons();
    defmt::trace!("Reset reasons: {:?}", defmt::Debug2Format(&rr));

    let vbus_present = pmic.lock().await.get_vbus_in_status().await?.is_vbus_in_present;

    let woke_by_gpio = rr.contains(ResetReason::RESETPIN);
    let woke_by_vbus = rr.contains(ResetReason::VBUS);

    // Errata 3.5 (55)
    if woke_by_gpio {
        defmt::trace!("Woke by GPIO (power button) -> continue boot");
        return Ok(());
    }

    if woke_by_vbus || (rr.is_empty() && vbus_present) {
        if vbus_present {
            defmt::trace!("VBUS present at boot -> charging LEDs + faux off");
            leds.set_state(pmic_leds::LedState::BattCharging).await;
        } else {
            defmt::trace!("No VBUS -> LEDs off + faux off");
            leds.set_state(pmic_leds::LedState::Off).await;
        }

        power::set_pending_ship_flag();

        return Ok(());
    }

    Ok(())
}
