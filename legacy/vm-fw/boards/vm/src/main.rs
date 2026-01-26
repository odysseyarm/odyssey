#![no_main]
#![no_std]
#![feature(never_type)]

use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;
use embassy_nrf::bind_interrupts;
use embassy_nrf::{
    config::{Config, Reg0Voltage},
    interrupt,
    nvmc::Nvmc,
    pac, peripherals, spim,
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::Timer;
use static_cell::StaticCell;

use common::{
    Paj,
    object_mode::{ObjectModeContext, object_mode},
    settings, usb,
};
use vm_board::{VmPlatform, imu, l2cap::VmL2capChannels, pins::Board, split_board, utils};

const DEVICE_INTERFACE_GUID: &str = "{4d36e96c-e325-11ce-bfc1-08002be10318}";

// Interrupt bindings for nRF52840 peripherals
bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler;
    TWISPI0 => embassy_nrf::spim::InterruptHandler<peripherals::TWISPI0>;
    TWISPI1 => embassy_nrf::spim::InterruptHandler<peripherals::TWISPI1>;
    SPI2 => embassy_nrf::spim::InterruptHandler<peripherals::SPI2>;
    TIMER1 => TimerIrq;
});

struct TimerIrq;

impl
    interrupt::typelevel::Handler<
        <embassy_nrf::peripherals::TIMER1 as embassy_nrf::timer::Instance>::Interrupt,
    > for TimerIrq
{
    unsafe fn on_interrupt() {
        let regs = unsafe { embassy_nrf::pac::timer::Timer::from_ptr(pac::TIMER1.as_ptr()) };
        regs.events_compare(1).write(|w| *w = 0);
        common::fodtrigger::FOD_TICK_SIG.signal(());
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    defmt::info!("VM board initialization starting...");

    // 1. Initialize embassy with VM-specific config
    let mut config = Config::default();
    config.dcdc.reg0_voltage = Some(Reg0Voltage::_2V4);
    let p = embassy_nrf::init(config);

    defmt::info!("Embassy initialized with DCDC at 2.4V");

    // 2. Split board peripherals using the VM pin definitions
    let board: Board = split_board!(p);

    // 3. Initialize USB device
    defmt::info!("Initializing USB device...");
    let vbus = embassy_nrf::usb::vbus_detect::HardwareVbusDetect::new(Irqs);
    let (usb_dev, usb_snd, usb_rcv, usb_signal) = usb::usb_device(
        0x520F, // USB VM PID
        DEVICE_INTERFACE_GUID,
        p.USBD,
        Irqs,
        vbus,
        |_| {},
    );
    spawner.spawn(usb::run_usb(usb_dev)).unwrap();
    defmt::info!("USB device spawned");

    // 4. Initialize PAJ7025 sensors (R2 and R3)
    defmt::info!("Initializing PAJ7025 sensors...");

    // PAJ7025 R2 (Near Field) on TWISPI0
    let paj_r2_spi_pins = common::utils::SpiPins {
        sck: board.paj7025r2.sck.into(),
        miso: board.paj7025r2.miso.into(),
        mosi: board.paj7025r2.mosi.into(),
        cs: board.paj7025r2.cs.into(),
    };
    let mut paj_r2_config = spim::Config::default();
    paj_r2_config.frequency = spim::Frequency::M8;
    paj_r2_config.mode = spim::MODE_3;
    paj_r2_config.bit_order = spim::BitOrder::LSB_FIRST;

    // PAJ7025 R3 (Wide Field) on TWISPI1
    let paj_r3_spi_pins = common::utils::SpiPins {
        sck: board.paj7025r3.sck.into(),
        miso: board.paj7025r3.miso.into(),
        mosi: board.paj7025r3.mosi.into(),
        cs: board.paj7025r3.cs.into(),
    };
    let mut paj_r3_config = spim::Config::default();
    paj_r3_config.frequency = spim::Frequency::M8;
    paj_r3_config.mode = spim::MODE_3;
    paj_r3_config.bit_order = spim::BitOrder::LSB_FIRST;

    let paj_r2_dev = common::utils::make_spi_dev(p.TWISPI0, Irqs, paj_r2_spi_pins, paj_r2_config);
    let paj_r3_dev = common::utils::make_spi_dev(p.TWISPI1, Irqs, paj_r3_spi_pins, paj_r3_config);

    // Allow the sensors to settle after SPI setup before issuing commands.
    Timer::after_millis(1).await;

    let mut paj_r2 = paj7025::Paj7025::init(paj_r2_dev).await.unwrap();
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

    static PAJ_R2: StaticCell<AsyncMutex<NoopRawMutex, Paj>> = StaticCell::new();
    let paj_r2_mutex = PAJ_R2.init(AsyncMutex::new(paj_r2));
    defmt::info!("PAJ7025 R2 (NF) initialized");

    let mut paj_r3 = paj7025::Paj7025::init(paj_r3_dev).await.unwrap();
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

    static PAJ_R3: StaticCell<AsyncMutex<NoopRawMutex, Paj>> = StaticCell::new();
    let paj_r3_mutex = PAJ_R3.init(AsyncMutex::new(paj_r3));
    defmt::info!("PAJ7025 R3 (WF) initialized");

    defmt::info!("FOD trigger channels initialized");

    // 6. Initialize BMI270 IMU on SPI2
    defmt::info!("Initializing BMI270 IMU...");
    let imu_spi_pins = common::utils::SpiPins {
        sck: board.bmi270.sck.into(),
        miso: board.bmi270.miso.into(),
        mosi: board.bmi270.mosi.into(),
        cs: board.bmi270.cs.into(),
    };

    let (imu, imu_int) =
        imu::init::<_, _, 2048, 256>(p.SPI2, Irqs, imu_spi_pins, board.bmi270.irq.into())
            .await
            .unwrap();

    defmt::info!("BMI270 IMU initialized");

    // 7. Initialize settings storage with NVMC
    defmt::info!("Initializing settings storage...");
    let nvmc = Nvmc::new(p.NVMC);
    let flash_dev = embassy_embedded_hal::adapter::BlockingAsync::new(nvmc);

    let settings_ref = settings::init_settings(
        &spawner,
        flash_dev,
        &mut *paj_r2_mutex.lock().await,
        &mut *paj_r3_mutex.lock().await,
    )
    .await
    .unwrap();

    defmt::info!("Settings initialized and applied to PAJ sensors");

    // 8. Create VM object-mode context
    defmt::info!("Creating ObjectModeContext...");
    let ctx = ObjectModeContext::<VmPlatform> {
        paj7025r2_group: (
            paj_r2_mutex,
            board.paj7025r2.fod.into(),
            p.GPIOTE_CH1.into(),
        ),
        paj7025r3_group: (
            paj_r3_mutex,
            board.paj7025r3.fod.into(),
            p.GPIOTE_CH2.into(),
        ),
        fod_set_ch: p.PPI_CH1.into(),
        fod_clr_ch: p.PPI_CH2.into(),
        timer: p.TIMER1,
        imu,
        imu_int,
        usb_snd,
        usb_rcv,
        usb_configured: usb_signal,
        settings: settings_ref,
        l2cap_channels: VmL2capChannels::new(),
        data_mode: || common::protodongers::control::device::TransportMode::Usb,
        version: utils::FIRMWARE_VERSION,
    };

    defmt::info!("VM initialization complete! Entering object mode...");

    // 10. Enter object mode (runs forever)
    let _ = object_mode(0x520F, ctx).await;
}
