#![no_main]
#![no_std]

mod usb;
// TODO
// mod pins;

use bmi2::{bmi2_async::Bmi2, types::Burst};
use embassy_executor::Spawner;
use embassy_nrf::{
    bind_interrupts,
    gpio::{Level, Output, OutputDrive},
    peripherals,
    spim::{self, Spim},
    Peripherals,
};
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

type SpiDevR2 = ExclusiveDevice<Spim<'static, peripherals::TWISPI0>, Output<'static>, Delay>;
type SpiDevR3 = ExclusiveDevice<Spim<'static, peripherals::TWISPI1>, Output<'static>, Delay>;
type SpiDevB2 = ExclusiveDevice<Spim<'static, peripherals::SPI2>,    Output<'static>, Delay>;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p: Peripherals = embassy_nrf::init(Default::default());
    defmt::info!("boot");

    let mut cfg_paj = spim::Config::default();
    cfg_paj.frequency = spim::Frequency::M16;
    cfg_paj.mode = spim::MODE_3;
    cfg_paj.bit_order = spim::BitOrder::MSB_FIRST;

    let spim_r2 = Spim::new(p.TWISPI0, Irqs, p.P0_05, p.P0_04, p.P0_26, cfg_paj.clone());
    let cs_r2 = Output::new(p.P0_08, Level::High, OutputDrive::Standard);
    let dev_r2: SpiDevR2 = ExclusiveDevice::new(spim_r2, cs_r2, Delay).unwrap();

    let spim_r3 = Spim::new(p.TWISPI1, Irqs, p.P1_00, p.P0_22, p.P1_07, cfg_paj);
    let cs_r3 = Output::new(p.P0_06, Level::High, OutputDrive::Standard);
    let dev_r3: SpiDevR3 = ExclusiveDevice::new(spim_r3, cs_r3, Delay).unwrap();

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

    let spim_b2 = Spim::new(p.SPI2, Irqs, p.P0_20, p.P0_19, p.P0_15, cfg_bmi);
    let cs_b2 = Output::new(p.P0_03, Level::High, OutputDrive::Standard);
    let dev_b2: SpiDevB2 = ExclusiveDevice::new(spim_b2, cs_b2, Delay).unwrap();

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
