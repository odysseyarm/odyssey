use embassy_nrf::Peri;
use embassy_nrf::peripherals::USBD;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::{Driver as NrfUsbDriver, Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_sync::watch::{DynReceiver, Watch};
use embassy_usb::msos::windows_version;
use embassy_usb::{Builder, Config as UsbConfig, msos};
use static_cell::{ConstStaticCell, StaticCell};

use crate::Irqs;

pub type UsbDriver = NrfUsbDriver<'static, HardwareVbusDetect>;

pub const VID: u16 = 0x1915;
pub const PID: u16 = 0x5212;

const DEVICE_INTERFACE_GUIDS: &[&str] = &["{B0714FF4-0D68-4222-A73A-7CE3B8B9A601}"];

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

    // Serial number is the 64-bit FICR DEVICEID as 16 hex digits.
    static SERIAL_BUF: ConstStaticCell<[u8; 16]> = ConstStaticCell::new([0; 16]);
    let serial_buf = SERIAL_BUF.take();
    let ficr = embassy_nrf::pac::FICR;
    let low = ficr.deviceid(0).read().to_le_bytes();
    let high = ficr.deviceid(1).read().to_le_bytes();
    let id_bytes: [u8; 8] = [
        low[0], low[1], low[2], low[3], high[0], high[1], high[2], high[3],
    ];
    for (i, &b) in id_bytes.iter().enumerate() {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        serial_buf[i * 2] = HEX[(b >> 4) as usize];
        serial_buf[i * 2 + 1] = HEX[(b & 0xf) as usize];
    }
    // SAFETY: serial_buf holds only ASCII hex digits, which are valid UTF-8.
    let serial = unsafe { core::str::from_utf8_unchecked(serial_buf) };

    let mut config = UsbConfig::new(VID, PID);
    config.manufacturer = Some("Odyssey Arm");
    config.product = Some("Dongle USB Mux");
    config.serial_number = Some(serial);
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

    // Vendor-specific class 0xFF; the MS OS descriptors bind it to WinUSB.
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
    fn reset(&mut self) {
        defmt::warn!("USB bus reset");
        // Report deconfigured on bus reset so the tasks re-arm and re-enumeration
        // starts clean when the dongle is plugged in before the PC boots or after a
        // failed enumeration.
        USB_CONFIG_WATCH.sender().send(false);
        self.signal.signal(false);
    }

    fn addressed(&mut self, addr: u8) {
        defmt::info!("USB addressed: {}", addr);
    }

    fn configured(&mut self, configured: bool) {
        defmt::info!("USB configured: {}", configured);
        USB_CONFIG_WATCH.sender().send(configured);
        self.signal.signal(configured);
    }

    fn suspended(&mut self, suspended: bool) {
        defmt::info!("USB suspended: {}", suspended);
    }

    fn control_out(
        &mut self,
        req: embassy_usb::control::Request,
        data: &[u8],
    ) -> Option<embassy_usb::control::OutResponse> {
        use embassy_usb::control::{OutResponse, Recipient, RequestType};
        use odyssey_protocol::control::usb_mux::UsbMuxCtrlMsg;

        // Vendor|Interface, request SEND=0x30, wIndex=0
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
            defmt::trace!("Control IN: host polling for event");
            if let Some(msg) = crate::control::try_recv_event() {
                defmt::info!(
                    "Control IN: sending event to host: {:?}",
                    defmt::Debug2Format(&msg)
                );
                match postcard::to_slice(&msg, buf) {
                    Ok(out) => Some(InResponse::Accepted(out)),
                    Err(_) => {
                        defmt::error!("Control IN: failed to serialize event");
                        Some(InResponse::Rejected)
                    }
                }
            } else {
                defmt::trace!("Control IN: no event pending");
                // A zero-length reply means "no event".
                Some(InResponse::Accepted(&[]))
            }
        } else {
            None
        }
    }
}

// A Watch rather than a Signal because several tasks gate on it and Signal has
// a single consumer.
static USB_CONFIG_WATCH: Watch<ThreadModeRawMutex, bool, 4> = Watch::new();

pub fn usb_config_receiver() -> DynReceiver<'static, bool> {
    USB_CONFIG_WATCH
        .dyn_receiver()
        .expect("usb config watch receivers exhausted")
}
