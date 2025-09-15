use ariel_nrf::usb::{Driver, vbus_detect::HardwareVbusDetect};
use ariel_os::{
    cell::StaticCell, debug::log::{Hex, info}, delegate::Delegate, reexports::embassy_usb, usb::{UsbBuilder, UsbDriver}
};
use embassy_usb::{
    class::cdc_acm::{CdcAcmClass, State},
    driver::EndpointError,
};

const MAX_FULL_SPEED_PACKET_SIZE: u8 = 64;

#[ariel_os::config(usb)]
const USB_CONFIG: embassy_usb::Config<'_> = {
    let mut config = embassy_usb::Config::new(0x1915, 0x520F);
    config.manufacturer = Some("Odyssey Arm");
    config.product = Some("ATS USB Legacy");
    // config.serial_number = Some("12345678");
    config.max_power = 100;
    config.max_packet_size_0 = MAX_FULL_SPEED_PACKET_SIZE;

    // Required for Windows support.
    config.composite_with_iads = true;
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config
};

type StaticCdcAcmClass = CdcAcmClass<'static, Driver<'static, ariel_nrf::peripherals::USBD, HardwareVbusDetect>>;

pub async fn usb_device(delegate: &Delegate<UsbBuilder>) -> StaticCdcAcmClass {
    info!("Hello World!");

    static STATE: StaticCell<State<'_>> = StaticCell::new();

    // Create and inject the USB class on the system USB builder.
    delegate
        .with(|builder| {
            CdcAcmClass::new(
                builder,
                STATE.init_with(State::new),
                MAX_FULL_SPEED_PACKET_SIZE.into(),
            )
        })
        .await
}

pub async fn echo_srv(mut class: StaticCdcAcmClass) -> ! {
    loop {
        class.wait_connection().await;
        info!("Connected");
        let _ = echo(&mut class).await;
        info!("Disconnected");
    }
}

struct Disconnected {}

impl From<EndpointError> for Disconnected {
    fn from(val: EndpointError) -> Self {
        match val {
            EndpointError::BufferOverflow => panic!("Buffer overflow"),
            EndpointError::Disabled => Disconnected {},
        }
    }
}

async fn echo(class: &mut CdcAcmClass<'static, UsbDriver>) -> Result<(), Disconnected> {
    let mut buf = [0; MAX_FULL_SPEED_PACKET_SIZE as usize];
    loop {
        let n = class.read_packet(&mut buf).await?;
        let data = &buf[..n];
        info!("data: {}", Hex(data));
        class.write_packet(data).await?;
    }
}
