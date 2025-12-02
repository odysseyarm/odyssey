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
pub const PID: u16 = 0x520F;

const DEVICE_INTERFACE_GUIDS: &[&str] = &["{4d36e96c-e325-11ce-bfc1-08002be10318}"];

#[embassy_executor::task]
pub async fn run_usb(mut dev: StaticUsbDevice) -> ! {
    dev.run().await
}

static USB_CONFIGURED: AtomicBool = AtomicBool::new(false);

pub fn usb_device<Irqs>(
    usbd: Peri<'static, USBD>,
    irqs: Irqs,
    vbus: HardwareVbusDetect,
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

    let mut config = UsbConfig::new(VID, PID);
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

    static SIGNAL: ConstStaticCell<Signal<ThreadModeRawMutex, bool>> = ConstStaticCell::new(Signal::new());
    let signal = SIGNAL.take();
    USB_CONFIGURED.store(false, Ordering::Release);

    static HANDLER: StaticCell<MyHandler> = StaticCell::new();
    let handler = HANDLER.init(MyHandler::new(signal, &USB_CONFIGURED));

    builder.handler(&mut *handler);

    // Add a vendor-specific function (class 0xFF), and corresponding interface,
    // that uses our custom handler.
    let mut function = builder.function(0xFF, 0, 0);
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
