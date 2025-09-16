#![no_main]
#![no_std]

mod usb;
mod pins;

use bmi2::bmi2_async::Bmi2;
use bmi2::types::Burst;
use embassy_executor::Spawner;
use embassy_nrf::config::Config;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::interrupt::typelevel::Binding;
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::{Peri, bind_interrupts, peripherals};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use paj7025::Paj7025;
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

    // ========= PAG7025 (R2/R3) on TWISPI0 / TWISPI1 =========
    let mut cfg_paj = spim::Config::default();
    cfg_paj.frequency = spim::Frequency::M16;
    cfg_paj.mode = spim::MODE_3;
    cfg_paj.bit_order = spim::BitOrder::LSB_FIRST;

    let dev_r2 = make_spi_dev(p.TWISPI0, Irqs, b.paj7025r2_spi.into(), cfg_paj.clone());
    let dev_r3 = make_spi_dev(p.TWISPI1, Irqs, b.paj7025r3_spi.into(), cfg_paj);
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
    defmt::info!("PAG7025 R2 Product ID: 0x{:x}", prod_id_r2);

    let prod_id_r3 = paj7025r3
        .ll
        .control()
        .bank_0()
        .product_id()
        .read_async()
        .await
        .unwrap()
        .value();
    defmt::info!("PAG7025 R3 Product ID: 0x{:x}", prod_id_r3);

    let mut cfg_bmi = spim::Config::default();
    cfg_bmi.frequency = spim::Frequency::M8;
    cfg_bmi.mode = spim::MODE_0;
    cfg_bmi.bit_order = spim::BitOrder::MSB_FIRST;
    
    let dev_b2 = make_spi_dev(p.SPI2, Irqs, b.bmi270_spi.into(), cfg_bmi);

    const FIFO_BURST: usize = 256;
    let mut bmi: Bmi2<_, _, FIFO_BURST> = Bmi2::new_spi(dev_b2, Delay, Burst::new(255));
    let chip_id = bmi.get_chip_id().await.unwrap();
    defmt::info!("BMI270 Chip ID: 0x{:x}", chip_id);

    let (mut cdc, usb) = usb::usb_device(p.USBD);
    spawner.must_spawn(usb::run_usb(usb));

    cdc.wait_connection().await;
    defmt::info!("CDC-ACM connected");

    let (mut tx, mut rx, _ctl) = cdc.split_with_control();
    loop {
        let mut buf = [0u8; 64];
        let n = rx.read_packet(&mut buf).await.unwrap();
        if n == 0 {
            continue;
        }
        tx.write_packet(&buf[..n]).await.unwrap();
    }
}

fn make_spi_dev<SPI, IRQ>(
    spi: Peri<'static, SPI>,
    irqs: IRQ,
    pins: pins::Spi,
    cfg: spim::Config,
) -> ExclusiveDevice<Spim<'static, SPI>, Output<'static>, Delay>
where
    SPI: spim::Instance,
    IRQ: Binding<<SPI as spim::Instance>::Interrupt, spim::InterruptHandler<SPI>> + 'static,
{
    let spim = Spim::new(spi, irqs, pins.sck, pins.miso, pins.mosi, cfg);
    let cs_out = Output::new(pins.cs, Level::High, OutputDrive::Standard);
    ExclusiveDevice::new(spim, cs_out, Delay).unwrap()
}
