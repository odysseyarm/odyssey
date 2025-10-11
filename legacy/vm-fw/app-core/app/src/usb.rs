use embassy_nrf::Peri;
use embassy_nrf::peripherals::USBD;
use embassy_nrf::usb::{Driver as NrfUsbDriver, Endpoint, In, Out};
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_usb::msos::windows_version;
use embassy_usb::{Config as UsbConfig, UsbDevice, msos};
use static_cell::ConstStaticCell;

use crate::Irqs;

pub type UsbDriver = NrfUsbDriver<'static, HardwareVbusDetect>;
pub type StaticUsbDevice = UsbDevice<'static, UsbDriver>;

pub const VID: u16 = 0x1915;
pub const PID: u16 = 0x520F;

const DEVICE_INTERFACE_GUIDS: &[&str] = &["{4d36e96c-e325-11ce-bfc1-08002be10318}"];

#[embassy_executor::task]
pub async fn run_usb(mut dev: StaticUsbDevice) -> ! {
    dev.run().await
}

pub fn usb_device(usbd: Peri<'static, USBD>) -> (StaticUsbDevice, Endpoint<'static, In>, Endpoint<'static, Out>) {
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

    // Add the Microsoft OS Descriptor (MSOS/MOD) descriptor.
    // We tell Windows that this entire device is compatible with the "WINUSB" feature,
    // which causes it to use the built-in WinUSB driver automatically, which in turn
    // can be used by libusb/rusb software without needing a custom driver or INF file.
    // In principle you might want to call msos_feature() just on a specific function,
    // if your device also has other functions that still use standard class drivers.
    builder.msos_descriptor(windows_version::WIN8_1, 0);
    builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
    ));

    // Add a vendor-specific function (class 0xFF), and corresponding interface,
    // that uses our custom handler.
    let mut function = builder.function(0xFF, 0, 0);
    let mut interface = function.interface();
    let mut alt = interface.alt_setting(0xFF, 0, 0, None);
    let ep_out = alt.endpoint_bulk_out(None, 64);
    let ep_in = alt.endpoint_bulk_in(None, 64);
    drop(function);

    let usb = builder.build();
    (usb, ep_in, ep_out)
}
