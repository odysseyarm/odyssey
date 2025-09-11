#![no_main]
#![no_std]

mod peripherals;

use ariel_os::{
    debug::{exit, log::{debug, info}, ExitCode},
    gpio, hal,
    spi::{main::{highest_freq_in, Kilohertz, SpiDevice}, Mode},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use paj7025::Paj7025;
use static_cell::StaticCell;

static SPI_BUS_R2: StaticCell<Mutex<CriticalSectionRawMutex, hal::spi::main::Spi>> = StaticCell::new();
static SPI_BUS_R3: StaticCell<Mutex<CriticalSectionRawMutex, hal::spi::main::Spi>> = StaticCell::new();

fn make_spi_device(
    bus: hal::spi::main::Spi,
    cs_output: gpio::Output,
    cell: &'static StaticCell<Mutex<CriticalSectionRawMutex, hal::spi::main::Spi>>,
) -> SpiDevice {
    let bus_mutex: &'static Mutex<CriticalSectionRawMutex, hal::spi::main::Spi> =
        cell.init(Mutex::new(bus));

    SpiDevice::new(bus_mutex, cs_output)
}

#[ariel_os::task(autostart, peripherals)]
async fn main(peripherals: peripherals::Peripherals) {
    info!(
        "Hello from main()! Running on a {} board.",
        ariel_os::buildinfo::BOARD
    );

    let mut spi_config = hal::spi::main::Config::default();
    spi_config.frequency = const { highest_freq_in(Kilohertz::kHz(100)..=Kilohertz::MHz(14)) };
    spi_config.mode = Mode::Mode3;
    debug!("Selected frequency: {:?}", spi_config.frequency);

    // Convert concrete pin-sets into the unified `pins::Spi`.
    let r2 = peripherals::Spi::from(peripherals.paj7025r2_spi);
    let r3 = peripherals::Spi::from(peripherals.paj7025r3_spi);

    // Build devices and then init the sensors.
    let spi = ariel_os::hal::spi::main::TWISPI0::new(r2.sck, r2.miso, r2.mosi, spi_config.clone());
    let cs_output = gpio::Output::new(r2.cs, gpio::Level::High);
    let dev_r2 = make_spi_device(spi, cs_output, &SPI_BUS_R2);
    let spi = ariel_os::hal::spi::main::TWISPI1::new(r3.sck, r3.miso, r3.mosi, spi_config);
    let cs_output = gpio::Output::new(r3.cs, gpio::Level::High);
    let dev_r3 = make_spi_device(spi, cs_output, &SPI_BUS_R3);

    let mut paj7025r2 = Paj7025::init(dev_r2).await.unwrap();
    let _paj7025r3 = Paj7025::init(dev_r3).await.unwrap();

    // `ll` is a field:
    let _prod_id = paj7025r2
        .ll
        .control()
        .bank_0()
        .product_id()
        .read_async()
        .await
        .unwrap();

    exit(ExitCode::SUCCESS);
}
