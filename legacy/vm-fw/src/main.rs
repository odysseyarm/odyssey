#![no_main]
#![no_std]

mod pins;
mod usb;

use ariel_os::{
    debug::{
        log::{debug, info},
    },
    gpio, hal,
    spi::{
        Mode,
        main::{Kilohertz, SpiDevice, highest_freq_in},
    },
};
use bmi2::{bmi2_async::Bmi2, types::Burst};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use paj7025::Paj7025;
use static_cell::StaticCell;

use crate::usb::{echo_srv, usb_device};

static SPI_BUS_R2: StaticCell<Mutex<CriticalSectionRawMutex, hal::spi::main::Spi>> =
    StaticCell::new();
static SPI_BUS_R3: StaticCell<Mutex<CriticalSectionRawMutex, hal::spi::main::Spi>> =
    StaticCell::new();
static SPI_BUS_B2: StaticCell<Mutex<CriticalSectionRawMutex, hal::spi::main::Spi>> =
    StaticCell::new();

fn make_spi_device(
    bus: hal::spi::main::Spi,
    cs_output: gpio::Output,
    cell: &'static StaticCell<Mutex<CriticalSectionRawMutex, hal::spi::main::Spi>>,
) -> SpiDevice {
    let bus_mutex: &'static Mutex<CriticalSectionRawMutex, hal::spi::main::Spi> =
        cell.init(Mutex::new(bus));

    SpiDevice::new(bus_mutex, cs_output)
}

#[ariel_os::task(autostart, peripherals, usb_builder_hook)]
async fn main(pins: pins::Peripherals) {
    info!(
        "Hello from main()! Running on a {} board.",
        ariel_os::buildinfo::BOARD
    );

    let mut spi_config = hal::spi::main::Config::default();
    spi_config.frequency = const { highest_freq_in(Kilohertz::kHz(100)..=Kilohertz::MHz(14)) };
    spi_config.mode = Mode::Mode3;
    spi_config.bit_order = ariel_os_embassy_common::spi::BitOrder::LsbFirst;
    debug!("Selected frequency: {:?}", spi_config.frequency);

    let r2 = pins::Spi::from(pins.paj7025r2_spi);
    let r3 = pins::Spi::from(pins.paj7025r3_spi);
    let b2 = pins::Spi::from(pins.bmi270_spi);

    let spi = ariel_os::hal::spi::main::TWISPI0::new(r2.sck, r2.miso, r2.mosi, spi_config.clone());
    let cs_output = gpio::Output::new(r2.cs, gpio::Level::High);
    let dev_r2 = make_spi_device(spi, cs_output, &SPI_BUS_R2);
    let spi = ariel_os::hal::spi::main::TWISPI1::new(r3.sck, r3.miso, r3.mosi, spi_config.clone());
    let cs_output = gpio::Output::new(r3.cs, gpio::Level::High);
    let dev_r3 = make_spi_device(spi, cs_output, &SPI_BUS_R3);

    let mut paj7025r2 = Paj7025::init(dev_r2).await.unwrap();
    let mut paj7025r3 = Paj7025::init(dev_r3).await.unwrap();

    let prod_id = paj7025r2
        .ll
        .control()
        .bank_0()
        .product_id()
        .read_async()
        .await
        .unwrap()
        .value();

    info!("Product ID for r2: {:#X}", prod_id);

    let prod_id = paj7025r3
        .ll
        .control()
        .bank_0()
        .product_id()
        .read_async()
        .await
        .unwrap()
        .value();

    info!("Product ID for r3: {:#X}", prod_id);

    spi_config.frequency = const { highest_freq_in(Kilohertz::kHz(100)..=Kilohertz::MHz(10)) };
    spi_config.mode = Mode::Mode0;
    spi_config.bit_order = ariel_os_embassy_common::spi::BitOrder::MsbFirst;
    let cs_output = gpio::Output::new(b2.cs, gpio::Level::High);
    let spi = ariel_os::hal::spi::main::SPI2::new(b2.sck, b2.miso, b2.mosi, spi_config);
    let spi = make_spi_device(spi, cs_output, &SPI_BUS_B2);
    const BUFFER_SIZE: usize = 256;
    let mut bmi: Bmi2<_, _, BUFFER_SIZE> = Bmi2::new_spi(
        spi,
        ariel_os_embassy_common::reexports::embassy_time::Delay,
        Burst::new(255),
    );
    let chip_id = bmi.get_chip_id().await.unwrap();
    info!("Chip ID for BMI270: {:#X}", chip_id);
    
    let class = usb_device(&USB_BUILDER_HOOK).await;
    echo_srv(class).await;
}
