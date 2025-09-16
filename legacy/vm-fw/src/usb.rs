use embassy_nrf::peripherals::USBD;
use embassy_nrf::usb::{vbus_detect::HardwareVbusDetect, Driver as NrfUsbDriver};
use embassy_nrf::Peri;

use embassy_usb::class::cdc_acm::{self, CdcAcmClass, State as CdcState};
use embassy_usb::{Builder, Config as UsbConfig, UsbDevice};

use static_cell::StaticCell;

use crate::Irqs;

pub type UsbDriver = NrfUsbDriver<'static, USBD, HardwareVbusDetect>;
pub type StaticUsbDevice = UsbDevice<'static, UsbDriver>;
pub type StaticCdcAcmClass = CdcAcmClass<'static, UsbDriver>;

static DEVICE_DESC: StaticCell<[u8; 256]> = StaticCell::new();
static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
static BOS_DESC:    StaticCell<[u8; 256]> = StaticCell::new();
static MSOS_DESC:   StaticCell<[u8; 256]> = StaticCell::new();

static CDC_STATE: StaticCell<CdcState<'static>> = StaticCell::new();

pub fn usb_device(usbd: Peri<'static, USBD>) -> (StaticCdcAcmClass, StaticUsbDevice) {
    let vbus = HardwareVbusDetect::new(Irqs);
    let driver = UsbDriver::new(usbd, Irqs, vbus);

    let mut cfg = UsbConfig::new(0x1915, 0x520F);
    cfg.manufacturer = Some("Odyssey Arm");
    cfg.product = Some("ATS USB Legacy");
    // cfg.serial_number = Some("...");
    cfg.max_power = 100;
    cfg.max_packet_size_0 = 64;
    cfg.composite_with_iads = true;
    cfg.device_class = 0xEF;
    cfg.device_sub_class = 0x02;
    cfg.device_protocol = 0x01;

    let mut builder = Builder::new(
        driver,
        cfg,
        DEVICE_DESC.init([0; 256]),
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 256]),
    );

    let cdc_state = CDC_STATE.init(cdc_acm::State::new());
    let cdc = CdcAcmClass::new(&mut builder, cdc_state, 64);

    let usb = builder.build();
    (cdc, usb)
}

#[embassy_executor::task]
pub async fn run_usb(mut dev: StaticUsbDevice) -> ! {
    dev.run().await
}
