use embassy_nrf::Peri;
use embassy_nrf::peripherals::USBD;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::{Driver as NrfUsbDriver, Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_usb::msos::windows_version;
use embassy_usb::{Builder, Config as UsbConfig, msos};
use static_cell::{ConstStaticCell, StaticCell};

use crate::Irqs;

pub type UsbDriver = NrfUsbDriver<'static, HardwareVbusDetect>;

pub const VID: u16 = 0x1915;
pub const PID: u16 = 0x5210; // Dongle PID

const DEVICE_INTERFACE_GUIDS: &[&str] = &["{4d36e96c-e325-11ce-bfc1-08002be10318}"];

macro_rules! static_byte_buffer {
    ($size:expr) => {{
        static BUFFER: ::static_cell::ConstStaticCell<[u8; $size]> =
            ::static_cell::ConstStaticCell::new([0; $size]);
        BUFFER.take()
    }};
}

pub fn usb_device(
    usbd: Peri<'static, USBD>,
) -> (
    Builder<'static, UsbDriver>,
    Endpoint<'static, In>,
    Endpoint<'static, Out>,
    &'static Signal<ThreadModeRawMutex, bool>,
) {
    let vbus = HardwareVbusDetect::new(Irqs);
    let driver = UsbDriver::new(usbd, Irqs, vbus);

    let mut config = UsbConfig::new(VID, PID);
    config.manufacturer = Some("Odyssey Arm");
    config.product = Some("Dongle USB Mux");
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

    builder.msos_descriptor(windows_version::WIN8_1, 0);

    static SIGNAL: ConstStaticCell<Signal<ThreadModeRawMutex, bool>> =
        ConstStaticCell::new(Signal::new());
    let signal = SIGNAL.take();

    static HANDLER: StaticCell<MyHandler> = StaticCell::new();
    let handler = HANDLER.init(MyHandler::new(signal));

    builder.handler(&mut *handler);

    // Add a vendor-specific function (class 0xFF)
    let mut function = builder.function(0xFF, 0, 0);
    function.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    function.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
    ));
    let mut interface = function.interface();
    let mut alt = interface.alt_setting(0xFF, 0, 0, None);
    let ep_out = alt.endpoint_bulk_out(Some(1u8.into()), 64);
    let ep_in = alt.endpoint_bulk_in(Some(1u8.into()), 64);
    drop(function);
    (builder, ep_in, ep_out, signal)
}

struct MyHandler {
    signal: &'static Signal<ThreadModeRawMutex, bool>,
}

impl MyHandler {
    pub fn new(signal: &'static Signal<ThreadModeRawMutex, bool>) -> Self {
        Self { signal }
    }
}

impl embassy_usb::Handler for MyHandler {
    fn configured(&mut self, configured: bool) {
        self.signal.signal(configured);
    }

    fn control_out(
        &mut self,
        req: embassy_usb::control::Request,
        data: &[u8],
    ) -> Option<embassy_usb::control::OutResponse> {
        use embassy_usb::control::{OutResponse, Recipient, RequestType};
        use protodongers::control::usb_mux::UsbMuxCtrlMsg;

        // Vendor|Interface, request SEND=0x30, wIndex=1
        if req.request_type == RequestType::Vendor
            && req.recipient == Recipient::Interface
            && req.request == 0x30
            && req.index == 0
        {
            match postcard::from_bytes::<UsbMuxCtrlMsg>(data) {
                Ok(msg) => {
                    crate::control::try_send_cmd(msg);
                    Some(OutResponse::Accepted)
                }
                Err(_) => Some(OutResponse::Rejected),
            }
        } else {
            None
        }
    }

    fn control_in<'a>(
        &'a mut self,
        req: embassy_usb::control::Request,
        buf: &'a mut [u8],
    ) -> Option<embassy_usb::control::InResponse<'a>> {
        use embassy_usb::control::{InResponse, Recipient, RequestType};

        // Vendor|Interface, request RECV=0x31, wIndex=0
        if req.request_type == RequestType::Vendor
            && req.recipient == Recipient::Interface
            && req.request == 0x31
            && req.index == 0
        {
            if let Some(msg) = crate::control::try_recv_event() {
                match postcard::to_slice(&msg, buf) {
                    Ok(out) => Some(InResponse::Accepted(out)),
                    Err(_) => Some(InResponse::Rejected),
                }
            } else {
                // No event pending: return zero-length success to indicate "no data".
                Some(InResponse::Accepted(&[]))
            }
        } else {
            None
        }
    }
}
