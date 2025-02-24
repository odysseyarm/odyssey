use embassy_nrf::{
    Peripheral, peripherals,
    usb::{self, Driver, vbus_detect::HardwareVbusDetect},
};
use embassy_usb::{
    UsbDevice,
    class::cdc_acm::{self, CdcAcmClass},
};
use static_cell::{ConstStaticCell, StaticCell};

use crate::{Irqs, device_id_str};

pub async fn write_serial<'d, D: embassy_usb::driver::Driver<'d>>(
    class: &mut CdcAcmClass<'d, D>,
    data: &[u8],
) {
    let max_packet_size = usize::from(class.max_packet_size());
    for chunk in data.chunks(max_packet_size) {
        class.write_packet(chunk).await.unwrap();
    }
    if data.len() % max_packet_size == 0 {
        class.write_packet(&[]).await.unwrap();
    }
}

pub async fn wait_for_serial<'d, D: embassy_usb::driver::Driver<'d>>(
    class: &mut CdcAcmClass<'d, D>,
) -> u8 {
    loop {
        let mut buf = [0; 64];
        let Ok(_n) = class.read_packet(&mut buf).await else {
            continue;
        };
        return buf[0];
    }
}

#[embassy_executor::task]
pub async fn run_usb(
    mut device: UsbDevice<
        'static,
        embassy_nrf::usb::Driver<'static, embassy_nrf::peripherals::USBD, HardwareVbusDetect>,
    >,
) -> ! {
    device.run().await
}

/// Panics if called more than once.
pub fn usb_device(
    p: impl Peripheral<P = peripherals::USBD> + 'static,
) -> (
    CdcAcmClass<'static, Driver<'static, peripherals::USBD, HardwareVbusDetect>>,
    UsbDevice<'static, usb::Driver<'static, peripherals::USBD, HardwareVbusDetect>>,
) {
    // Create the driver, from the HAL.
    let driver = Driver::new(p, Irqs, HardwareVbusDetect::new(Irqs));

    // Create embassy-usb Config
    let mut config = embassy_usb::Config::new(0x1915, 0x5211);
    static SERIAL_NUMBER_BUFFER: ConstStaticCell<[u8; 16]> = ConstStaticCell::new([0; 16]);
    config.manufacturer = Some("Applied Math");
    config.product = Some("Thingo");
    config.serial_number = Some(device_id_str(SERIAL_NUMBER_BUFFER.take()));
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    // Create embassy-usb DeviceBuilder using the driver and config.
    // It needs some buffers for building the descriptors.
    static CONFIG_DESCRIPTOR: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
    static BOS_DESCRIPTOR: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
    static MSOS_DESCRIPTOR: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
    static CONTROL_BUF: ConstStaticCell<[u8; 64]> = ConstStaticCell::new([0; 64]);
    static STATE: StaticCell<cdc_acm::State> = StaticCell::new();

    let config_descriptor = CONFIG_DESCRIPTOR.take();
    let bos_descriptor = BOS_DESCRIPTOR.take();
    let msos_descriptor = MSOS_DESCRIPTOR.take();
    let control_buf = CONTROL_BUF.take();

    let state = STATE.init_with(cdc_acm::State::new);
    let mut builder = embassy_usb::Builder::new(
        driver,
        config,
        config_descriptor,
        bos_descriptor,
        msos_descriptor,
        control_buf,
    );

    // Create classes on the builder.
    let class = CdcAcmClass::new(&mut builder, state, 64);

    // Build the builder.
    let usb = builder.build();
    (class, usb)
}
