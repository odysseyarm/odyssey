use core::cmp::max;
use core::sync::atomic::{AtomicBool, Ordering};

use defmt::info;
use embassy_nrf::Peri;
use embassy_nrf::interrupt::typelevel::Binding;
use embassy_nrf::peripherals::USBD;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::{Driver as NrfUsbDriver, Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_usb::driver::{Endpoint as _, EndpointError, EndpointIn as _, EndpointOut as _};
use embassy_usb::msos::windows_version;
use embassy_usb::{Config as UsbConfig, UsbDevice, msos};
use static_cell::{ConstStaticCell, StaticCell};

pub struct UsbConfigHandle {
    signal: &'static Signal<ThreadModeRawMutex, bool>,
    flag: &'static AtomicBool,
}

impl Copy for UsbConfigHandle {}
impl Clone for UsbConfigHandle {
    fn clone(&self) -> Self {
        *self
    }
}

impl UsbConfigHandle {
    fn new(signal: &'static Signal<ThreadModeRawMutex, bool>, flag: &'static AtomicBool) -> Self {
        Self { signal, flag }
    }

    pub fn is_configured(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    pub async fn wait_until_configured(&self) {
        if self.is_configured() {
            return;
        }
        loop {
            let configured = self.signal.wait().await;
            if configured {
                return;
            }
        }
    }
}

use crate::utils::static_byte_buffer;

pub type UsbDriver = NrfUsbDriver<'static, HardwareVbusDetect>;
pub type StaticUsbDevice = UsbDevice<'static, UsbDriver>;

pub const VID: u16 = 0x1915;

const DEVICE_INTERFACE_GUIDS: &[&str] = &["{4d36e96c-e325-11ce-bfc1-08002be10318}"];

#[embassy_executor::task]
pub async fn run_usb(mut dev: StaticUsbDevice) -> ! {
    dev.run().await
}

static USB_CONFIGURED: AtomicBool = AtomicBool::new(false);

pub fn usb_device<Irqs>(
    pid: u16,
    usbd: Peri<'static, USBD>,
    irqs: Irqs,
    vbus: HardwareVbusDetect,
    builder_callback: impl FnOnce(&mut embassy_usb::Builder<'static, UsbDriver>),
) -> (
    StaticUsbDevice,
    Endpoint<'static, In>,
    Endpoint<'static, Out>,
    UsbConfigHandle,
)
where
    Irqs: Binding<embassy_nrf::interrupt::typelevel::USBD, embassy_nrf::usb::InterruptHandler<USBD>> + Copy + 'static,
{
    let driver = UsbDriver::new(usbd, irqs, vbus);

    let mut config = UsbConfig::new(VID, pid);
    config.manufacturer = Some("Odyssey Arm");
    config.product = Some("ATS USB Legacy");
    // config.serial_number = Some("...");
    config.max_power = 310;
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
    builder.msos_descriptor(windows_version::WIN8_1, 0);

    static SIGNAL: ConstStaticCell<Signal<ThreadModeRawMutex, bool>> = ConstStaticCell::new(Signal::new());
    let signal = SIGNAL.take();
    USB_CONFIGURED.store(false, Ordering::Release);

    static HANDLER: StaticCell<MyHandler> = StaticCell::new();
    let handler = HANDLER.init(MyHandler::new(signal, &USB_CONFIGURED));

    builder.handler(&mut *handler);

    // Call the builder callback to allow adding additional interfaces (e.g., DFU)
    builder_callback(&mut builder);

    // Add a vendor-specific function (class 0xFF), and corresponding interface,
    // that uses our custom handler.
    // We tell Windows that this specific function is compatible with the "WINUSB" feature,
    // which causes it to use the built-in WinUSB driver automatically, which in turn
    // can be used by libusb/rusb software without needing a custom driver or INF file.
    let mut function = builder.function(0xFF, 0, 0);
    function.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    function.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
    ));
    let mut interface = function.interface();
    let mut alt = interface.alt_setting(0xFF, 0, 0, None);
    let ep_out = alt.endpoint_bulk_out(None, 64);
    let ep_in = alt.endpoint_bulk_in(None, 64);
    drop(function);

    let usb = builder.build();
    let config_handle = UsbConfigHandle::new(signal, &USB_CONFIGURED);
    (usb, ep_in, ep_out, config_handle)
}

struct MyHandler {
    signal: &'static Signal<ThreadModeRawMutex, bool>,
    flag: &'static AtomicBool,
}

impl MyHandler {
    pub fn new(signal: &'static Signal<ThreadModeRawMutex, bool>, flag: &'static AtomicBool) -> Self {
        Self { signal, flag }
    }
}

impl embassy_usb::Handler for MyHandler {
    fn configured(&mut self, configured: bool) {
        self.flag.store(configured, Ordering::Release);
        info!("USB configured event: {}", configured);
        self.signal.signal(configured);
    }

    fn control_out(
        &mut self,
        req: embassy_usb::control::Request,
        data: &[u8],
    ) -> Option<embassy_usb::control::OutResponse> {
        use embassy_usb::control::{OutResponse, Recipient, RequestType};
        use protodongers::control::device::DeviceMsg;

        // Vendor|Interface, request SEND=0x30, wIndex=0
        if req.request_type == RequestType::Vendor
            && req.recipient == Recipient::Interface
            && req.request == 0x30
            && req.index == 0
        {
            match postcard::from_bytes::<DeviceMsg>(data) {
                Ok(msg) => {
                    crate::device_control::try_send_cmd(msg);
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
            if let Some(msg) = crate::device_control::try_recv_event() {
                match postcard::to_slice(&msg, buf) {
                    Ok(out) => Some(InResponse::Accepted(out)),
                    Err(_) => Some(InResponse::Rejected),
                }
            } else {
                // No event pending: return zero-length success
                Some(InResponse::Accepted(&[]))
            }
        } else {
            None
        }
    }
}

pub async fn read_transfer<'d>(ep: &mut Endpoint<'d, Out>, buf: &mut [u8]) -> Result<usize, EndpointError> {
    let max_packet = max(ep.info().max_packet_size, 1) as usize;
    let mut offset = 0;

    loop {
        if offset == buf.len() {
            return Err(EndpointError::BufferOverflow);
        }
        let n = ep.read(&mut buf[offset..]).await?;
        offset += n;

        if n < max_packet {
            break;
        }
    }

    Ok(offset)
}

pub async fn write_transfer<'d>(
    ep: &mut Endpoint<'d, In>,
    payload: &[u8],
    send_zlp: bool,
) -> Result<(), EndpointError> {
    let max_packet = max(ep.info().max_packet_size, 1) as usize;

    for chunk in payload.chunks(max_packet) {
        if !chunk.is_empty() {
            ep.write(chunk).await?;
        }
    }

    if send_zlp && payload.len() % max_packet == 0 {
        ep.write(&[]).await?;
    }

    Ok(())
}
