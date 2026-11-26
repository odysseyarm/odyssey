//! nRF52840 board assembly and register-to-image sensor handoff.

use embassy_executor::Spawner;
use embassy_nrf::config::{Config, Reg0Voltage};
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::{bind_interrupts, interrupt, peripherals, spim};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::Timer;
use legacy_common::fodtrigger::FodTrigger;
use legacy_common::object_mode::{ObjectModeContext, object_mode};
use legacy_common::{settings, usb};

use crate::l2cap::VmL2capChannels;
use crate::{VmPlatform, imu, utils};

const DEVICE_INTERFACE_GUID: &str = "{4d36e96c-e325-11ce-bfc1-08002be10318}";

// VM is bus-powered and has no fuel gauge.
static BATTERY_SOC: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(100);
static BATTERY_CHARGING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler;
    TWISPI0 => embassy_nrf::spim::InterruptHandler<peripherals::TWISPI0>, embassy_nrf::spis::InterruptHandler<peripherals::TWISPI0>;
    TWISPI1 => embassy_nrf::spim::InterruptHandler<peripherals::TWISPI1>, embassy_nrf::spis::InterruptHandler<peripherals::TWISPI1>;
    SPI2 => embassy_nrf::spim::InterruptHandler<peripherals::SPI2>;
    GPIOTE => embassy_nrf::gpio::InterruptHandler<peripherals::P0_25>;
    TIMER1 => TimerIrq;
});

struct TimerIrq;

#[expect(
    unsafe_code,
    reason = "interrupt Handler requires an unsafe callback signature"
)]
impl interrupt::typelevel::Handler<<peripherals::TIMER1 as embassy_nrf::timer::Instance>::Interrupt>
    for TimerIrq
{
    unsafe fn on_interrupt() {
        embassy_nrf::pac::TIMER1.events_compare(1).write_value(0);
        legacy_common::fodtrigger::FOD_TICK_SIG.signal(());
    }
}

/// Initializes VM hardware and serves USB tracking, then image mode until reset.
pub async fn run(spawner: Spawner) -> ! {
    let mut config = Config::default();
    config.dcdc.reg0_voltage = Some(Reg0Voltage::_2V4);
    let p = embassy_nrf::init(config);
    let board = split_board!(p);
    let vbus = embassy_nrf::usb::vbus_detect::HardwareVbusDetect::new(Irqs);
    let (usb_dev, usb_snd, usb_rcv, usb_signal) = usb::usb_device(
        0x520F,
        utils::device_id(),
        utils::FIRMWARE_VERSION,
        odyssey_version::cargo_semver!(),
        DEVICE_INTERFACE_GUID,
        p.USBD,
        Irqs,
        vbus,
        |_| {},
    );
    spawner.spawn(usb::run_usb(usb_dev).unwrap());

    let mut r2_bus = (
        p.TWISPI0,
        board.paj7025r2.sck,
        board.paj7025r2.mosi,
        board.paj7025r2.miso,
        board.paj7025r2.cs,
    );
    let mut r3_bus = (
        p.TWISPI1,
        board.paj7025r3.sck,
        board.paj7025r3.mosi,
        board.paj7025r3.miso,
        board.paj7025r3.cs,
    );
    let mut paj_config = spim::Config::default();
    paj_config.frequency = spim::Frequency::M8;
    paj_config.mode = spim::MODE_3;
    paj_config.bit_order = spim::BitOrder::LsbFirst;
    let dev_r2 = legacy_common::utils::make_spi_dev(
        r2_bus.0.reborrow(),
        Irqs,
        r2_bus.1.reborrow(),
        r2_bus.2.reborrow(),
        r2_bus.3.reborrow(),
        r2_bus.4.reborrow(),
        paj_config.clone(),
    );
    let dev_r3 = legacy_common::utils::make_spi_dev(
        r3_bus.0.reborrow(),
        Irqs,
        r3_bus.1.reborrow(),
        r3_bus.2.reborrow(),
        r3_bus.3.reborrow(),
        r3_bus.4.reborrow(),
        paj_config,
    );
    Timer::after_millis(1).await;
    let mut paj_r2 = paj7025::Paj7025::init(dev_r2).await.unwrap();
    let prod_id_r2 = paj_r2
        .ll
        .control()
        .bank_0()
        .product_id()
        .read_async()
        .await
        .unwrap()
        .value();
    defmt::info!("PAJ7025 R2 Product ID: 0x{:x}", prod_id_r2);
    paj_r2.init_settings(true, 0x10, 0x00).await.unwrap();
    let paj_r2_mutex = AsyncMutex::<NoopRawMutex, _>::new(paj_r2);
    let mut paj_r3 = paj7025::Paj7025::init(dev_r3).await.unwrap();
    let prod_id_r3 = paj_r3
        .ll
        .control()
        .bank_0()
        .product_id()
        .read_async()
        .await
        .unwrap()
        .value();
    defmt::info!("PAJ7025 R3 Product ID: 0x{:x}", prod_id_r3);
    paj_r3.init_settings(true, 0x10, 0x02).await.unwrap();
    let paj_r3_mutex = AsyncMutex::<NoopRawMutex, _>::new(paj_r3);

    let (imu, imu_int) = imu::init::<_, _, _, _, _, _, _, 2048, 256>(
        p.SPI2,
        Irqs,
        board.bmi270.sck,
        board.bmi270.mosi,
        board.bmi270.miso,
        board.bmi270.cs,
        board.bmi270.irq,
        settings::accel_odr(),
    )
    .await
    .unwrap();
    let flash_dev = embassy_embedded_hal::adapter::BlockingAsync::new(Nvmc::new(p.NVMC));
    let settings_ref = settings::init_settings(
        &spawner,
        flash_dev,
        &mut *paj_r2_mutex.lock().await,
        &mut *paj_r3_mutex.lock().await,
    )
    .await
    .unwrap();
    settings_ref.general.accel_config.accel_odr =
        imu::round_accel_odr_hz(settings_ref.general.accel_config.accel_odr);
    let ctx = ObjectModeContext::<VmPlatform> {
        paj7025r2: &paj_r2_mutex,
        paj7025r3: &paj_r3_mutex,
        fod: FodTrigger::new(
            board.paj7025r2.fod,
            board.paj7025r3.fod,
            p.GPIOTE_CH1,
            p.GPIOTE_CH2,
            p.TIMER1,
            p.PPI_CH1,
            p.PPI_CH2,
            embassy_nrf::timer::Frequency::F1MHz,
            5,
            2,
        ),
        imu,
        imu_int,
        usb_snd,
        usb_rcv,
        usb_configured: usb_signal,
        settings: settings_ref,
        l2cap_channels: VmL2capChannels::new(),
        data_mode: || legacy_common::odyssey_protocol::control::device::TransportMode::Usb,
        version: utils::FIRMWARE_VERSION,
        battery_soc: &BATTERY_SOC,
        battery_charging: &BATTERY_CHARGING,
    };
    let handoff = match object_mode(0x520F, ctx).await {
        Ok(handoff) => handoff,
        Err(error) => {
            defmt::error!(
                "object_mode exited with error: {:?}",
                defmt::Debug2Format(&error)
            );
            loop {
                Timer::after_secs(1).await;
            }
        }
    };
    // Register drivers must release their borrows before SPIS takes the same pins.
    drop(paj_r2_mutex);
    drop(paj_r3_mutex);
    legacy_common::image_mode::run(handoff, move |camera| {
        let mut config = embassy_nrf::spis::Config::default();
        config.mode = embassy_nrf::spis::MODE_3;
        if camera == legacy_common::odyssey_protocol::Port::Nf {
            let (serial, sck, mosi, _miso, cs) = r2_bus;
            embassy_nrf::spis::Spis::new_rxonly(serial, sck, mosi, cs, Irqs, config)
        } else {
            let (serial, sck, mosi, _miso, cs) = r3_bus;
            embassy_nrf::spis::Spis::new_rxonly(serial, sck, mosi, cs, Irqs, config)
        }
    })
    .await
}
