use embassy_nrf::Peri;
use embassy_nrf::peripherals::USBD;
use embassy_nrf::usb::Driver as NrfUsbDriver;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_usb::class::cdc_acm::{self, CdcAcmClass, State as CdcState};
use embassy_usb::driver::EndpointError;
use embassy_usb::{Config as UsbConfig, UsbDevice};
use static_cell::{ConstStaticCell, StaticCell};

use crate::Irqs;

pub type UsbDriver = NrfUsbDriver<'static, USBD, HardwareVbusDetect>;
pub type StaticUsbDevice = UsbDevice<'static, UsbDriver>;
pub type StaticCdcAcmClass = CdcAcmClass<'static, UsbDriver>;

pub const VID: u16 = 0x1915;
pub const PID: u16 = 0x520F;

static CDC_STATE: StaticCell<CdcState<'static>> = StaticCell::new();

pub async fn write_serial<'d, D: embassy_usb::driver::Driver<'d>>(snd: &mut cdc_acm::Sender<'d, D>, data: &[u8]) {
    let max_packet_size = usize::from(snd.max_packet_size());
    for chunk in data.chunks(max_packet_size) {
        snd.write_packet(chunk).await.unwrap();
    }
    if data.len().is_multiple_of(max_packet_size) {
        snd.write_packet(&[]).await.unwrap();
    }
}

pub async fn wait_for_serial<'d, D: embassy_usb::driver::Driver<'d>>(rcv: &mut cdc_acm::Receiver<'d, D>) -> u8 {
    loop {
        let mut buf = [0; 64];
        let r = rcv.read_packet(&mut buf).await;
        match r {
            Ok(_) => return buf[0],
            Err(EndpointError::Disabled) => {
                rcv.wait_connection().await;
            }
            Err(EndpointError::BufferOverflow) => unreachable!(),
        }
    }
}

#[embassy_executor::task]
pub async fn run_usb(mut dev: StaticUsbDevice) -> ! {
    dev.run().await
}

pub fn usb_device(usbd: Peri<'static, USBD>) -> (StaticCdcAcmClass, StaticUsbDevice) {
    let vbus = HardwareVbusDetect::new(Irqs);
    let driver = UsbDriver::new(usbd, Irqs, vbus);

    let mut config = UsbConfig::new(VID, PID);
    config.manufacturer = Some("Odyssey Arm");
    config.product = Some("ATS USB Legacy");
    // config.serial_number = Some("...");
    config.max_power = 100;
    config.max_packet_size_0 = 64;
    config.composite_with_iads = true;
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;

    let config_descriptor = static_byte_buffer!(256);
    let bos_descriptor = static_byte_buffer!(256);
    let msos_descriptor = static_byte_buffer!(256);
    let control_buf = static_byte_buffer!(64);

    let mut builder = embassy_usb::Builder::new(
        driver,
        config,
        config_descriptor,
        bos_descriptor,
        msos_descriptor,
        control_buf,
    );

    let cdc_state = CDC_STATE.init(cdc_acm::State::new());
    let cdc = CdcAcmClass::new(&mut builder, cdc_state, 64);

    let usb = builder.build();
    (cdc, usb)
}
