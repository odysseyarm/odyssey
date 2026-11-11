//! USB device setup for the nRF5340: a vendor-specific interface with bulk
//! endpoints, Microsoft OS 2.0 descriptors so Windows installs WinUSB without
//! an INF, and vendor control requests 0x30 (send) and 0x31 (receive) that
//! carry postcard-encoded `DeviceMsg`s.

mod connection_state;

use core::cmp::max;

use defmt::info;
use embassy_nrf::Peri;
use embassy_nrf::interrupt::typelevel::Binding;
use embassy_nrf::peripherals::USBD;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::usb::{Driver as NrfUsbDriver, Endpoint, In, Out};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::watch::Watch;
use embassy_usb::driver::{Endpoint as _, EndpointError, EndpointIn as _, EndpointOut as _};
use embassy_usb::msos::windows_version;
use embassy_usb::types::StringIndex;
use embassy_usb::{Config as UsbConfig, UsbDevice, msos};
use static_cell::{ConstStaticCell, StaticCell};

use self::connection_state::ConnectionState;
use crate::static_byte_buffer;

/// Most tasks that wait on the USB state at the same time. Each pending
/// `wait_until_*` call holds one receiver slot of the watch.
const USB_STATE_WAITERS: usize = 4;

type UsbActiveWatch = Watch<CriticalSectionRawMutex, ConnectionState, USB_STATE_WAITERS>;

/// Waits on whether a USB host is using the device (see [`is_active`]). Up to
/// [`USB_STATE_WAITERS`] tasks can wait at once.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Copy)]
pub struct UsbConfigHandle {
    watch: &'static UsbActiveWatch,
}

/// Whether a USB host is talking to the device: it has configured the device
/// and the bus is not suspended. False until [`usb_device`] has been called
/// and a host has enumerated the device.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn is_active() -> bool {
    USB_ACTIVE.try_get().is_some_and(|state| state.active)
}

impl UsbConfigHandle {
    /// Changes on each active-to-inactive USB boundary, including a reset or
    /// suspend that completes before a task next polls the configuration state.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn generation(&self) -> u32 {
        self.watch.try_get().map_or(0, |state| state.generation)
    }

    /// Waits for a connection boundary even if USB is already configured again.
    ///
    /// # Panics
    ///
    /// Panics if more than [`USB_STATE_WAITERS`] waits are pending at once.
    ///
    /// # Since
    /// Odyssey Desktop 0.7.0.
    pub async fn wait_until_generation_changes(&self, generation: u32) {
        let mut receiver = self
            .watch
            .receiver()
            .expect("no more than USB_STATE_WAITERS tasks wait on the USB state at once");
        receiver
            .get_and(|state| state.generation != generation)
            .await;
    }

    /// Returns once a host has configured the device and the bus is awake,
    /// immediately if that is already the case.
    ///
    /// # Panics
    ///
    /// Panics if more than [`USB_STATE_WAITERS`] waits are pending at once.
    ///
    /// # Since
    /// Odyssey Desktop 0.7.0.
    pub async fn wait_until_active(&self) {
        self.wait_for(true).await;
    }

    /// Returns as soon as the device is unplugged, reset, deconfigured or
    /// suspended, immediately if it already is.
    ///
    /// # Panics
    ///
    /// Panics if more than [`USB_STATE_WAITERS`] waits are pending at once.
    ///
    /// # Since
    /// Odyssey Desktop 0.7.0.
    pub async fn wait_until_inactive(&self) {
        self.wait_for(false).await;
    }

    async fn wait_for(&self, active: bool) {
        let mut receiver = self
            .watch
            .receiver()
            .expect("no more than USB_STATE_WAITERS tasks wait on the USB state at once");
        receiver.get_and(|state| state.active == active).await;
    }
}

/// Since: Odyssey Desktop 0.7.0.
pub type UsbDriver = NrfUsbDriver<'static, HardwareVbusDetect>;
/// Since: Odyssey Desktop 0.7.0.
pub type StaticUsbDevice = UsbDevice<'static, UsbDriver>;

/// Since: Odyssey Desktop 0.7.0.
pub const VID: u16 = 0x1915;

/// Since: Odyssey Desktop 0.7.0.
#[embassy_executor::task]
pub async fn run_usb(mut dev: StaticUsbDevice) -> ! {
    dev.run().await
}

static USB_ACTIVE: UsbActiveWatch = Watch::new_with(ConnectionState::INITIAL);

/// Called with each `DeviceMsg` the host sends with vendor request 0x30.
///
/// Since: Odyssey Desktop 0.7.0.
pub type DeviceControlSender = fn(odyssey_protocol::control::device::DeviceMsg);
/// Polled on vendor request 0x31. `None` answers with a zero-length response.
///
/// Since: Odyssey Desktop 0.7.0.
pub type DeviceControlReceiver = fn() -> Option<odyssey_protocol::control::device::DeviceMsg>;

/// Builds the composite USB device and returns it with the vendor interface's
/// bulk IN and OUT endpoints and a handle that tracks whether a host is using it.
///
/// The device uses IADs and Microsoft OS 2.0 descriptors so Windows installs
/// WinUSB automatically. Vendor control requests 0x30 and 0x31 go to
/// `device_control_send` and `device_control_recv`. `builder_callback` runs
/// before the vendor function is added and can add more interfaces, such as
/// DFU.
///
/// # Panics
///
/// Panics if called more than once: the descriptor buffers and handler are
/// statics.
///
/// # Since
/// Odyssey Desktop 0.7.0.
pub fn usb_device<Irqs>(
    pid: u16,
    firmware_version: [u32; 3],
    firmware_version_string: &'static str,
    product_name: &'static str,
    guid: &'static str,
    usbd: Peri<'static, USBD>,
    irqs: Irqs,
    vbus: HardwareVbusDetect,
    device_control_send: DeviceControlSender,
    device_control_recv: DeviceControlReceiver,
    builder_callback: impl FnOnce(&mut embassy_usb::Builder<'static, UsbDriver>),
) -> (
    StaticUsbDevice,
    Endpoint<'static, In>,
    Endpoint<'static, Out>,
    UsbConfigHandle,
)
where
    Irqs: Binding<embassy_nrf::interrupt::typelevel::USBD, embassy_nrf::usb::InterruptHandler<USBD>>
        + Copy
        + 'static,
{
    let driver = UsbDriver::new(usbd, irqs, vbus);

    static SERIAL_BUF: ConstStaticCell<[u8; 12]> = ConstStaticCell::new([0; 12]);
    let serial_buf = SERIAL_BUF.take();
    let ficr = embassy_nrf::pac::FICR;
    let low = ficr.info().deviceid(0).read().to_le_bytes();
    let high = ficr.info().deviceid(1).read().to_le_bytes();
    let id_bytes = [low[0], low[1], low[2], low[3], high[0], high[1]];
    for (i, &b) in id_bytes.iter().enumerate() {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        serial_buf[i * 2] = HEX[(b >> 4) as usize];
        serial_buf[i * 2 + 1] = HEX[(b & 0xf) as usize];
    }
    // SAFETY: serial_buf contains only ASCII hex digits
    let serial = unsafe { core::str::from_utf8_unchecked(serial_buf) };

    let mut config = UsbConfig::new(VID, pid);
    config.manufacturer = Some("Odyssey Arm");
    config.product = Some(product_name);
    config.serial_number = Some(serial);
    config.device_release = odyssey_version::firmware_bcd_device(firmware_version[0]);
    config.max_power = 310;
    config.max_packet_size_0 = 64;
    config.composite_with_iads = true;
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;

    let config_descriptor = static_byte_buffer!(256);
    let bos_descriptor = static_byte_buffer!(256);
    let msos_descriptor = static_byte_buffer!(512);
    let control_buf = static_byte_buffer!(256);

    let mut builder = embassy_usb::Builder::new(
        driver,
        config,
        config_descriptor,
        bos_descriptor,
        msos_descriptor,
        control_buf,
    );
    let firmware_version_index = builder.string();

    // For a composite device Windows needs the WINUSB features at both device
    // level and function level.
    builder.msos_descriptor(windows_version::WIN8_1, 0);
    builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(&[guid]),
    ));

    static HANDLER: StaticCell<MyHandler> = StaticCell::new();
    let handler = HANDLER.init(MyHandler {
        active: &USB_ACTIVE,
        configured: false,
        suspended: false,
        device_control_send,
        device_control_recv,
        firmware_version_index,
        firmware_version_string,
    });

    builder.handler(&mut *handler);

    builder_callback(&mut builder);

    // The WINUSB compatible ID on this vendor function (class 0xFF) makes
    // Windows bind its built-in WinUSB driver, which libusb/rusb can use
    // without a custom driver or INF file.
    let mut function = builder.function(0xFF, 0, 0);
    function.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    function.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(&[guid]),
    ));
    let mut interface = function.interface();
    let mut alt = interface.alt_setting(0xFF, 0, 0, None);
    let ep_out = alt.endpoint_bulk_out(None, 64);
    let ep_in = alt.endpoint_bulk_in(None, 64);
    drop(function);

    let usb = builder.build();
    let config_handle = UsbConfigHandle { watch: &USB_ACTIVE };
    (usb, ep_in, ep_out, config_handle)
}

struct MyHandler {
    active: &'static UsbActiveWatch,
    configured: bool,
    suspended: bool,
    device_control_send: DeviceControlSender,
    device_control_recv: DeviceControlReceiver,
    firmware_version_index: StringIndex,
    firmware_version_string: &'static str,
}

impl MyHandler {
    fn publish(&self) {
        let active = self.configured && !self.suspended;
        self.active.sender().send_if_modified(|state| {
            let mut next = state.unwrap_or(ConnectionState::INITIAL);
            let changed = next.set_active(active);
            *state = Some(next);
            changed
        });
    }
}

// embassy-usb reports `configured(false)` only for SET_CONFIGURATION 0. A bus
// reset and VBUS removal (unplug) arrive as `reset()` and `enabled(false)`, so
// those clear the state too.
impl embassy_usb::Handler for MyHandler {
    fn enabled(&mut self, enabled: bool) {
        if !enabled {
            info!("USB disabled");
            self.configured = false;
            self.suspended = false;
            self.publish();
        }
    }

    fn reset(&mut self) {
        info!("USB bus reset");
        self.configured = false;
        self.suspended = false;
        self.publish();
    }

    fn configured(&mut self, configured: bool) {
        info!("USB configured event: {}", configured);
        self.configured = configured;
        self.publish();
    }

    fn suspended(&mut self, suspended: bool) {
        info!("USB suspended: {}", suspended);
        self.suspended = suspended;
        self.publish();
    }

    fn get_string(&mut self, index: StringIndex, _lang_id: u16) -> Option<&str> {
        (index == self.firmware_version_index).then_some(self.firmware_version_string)
    }

    fn control_out(
        &mut self,
        req: embassy_usb::control::Request,
        data: &[u8],
    ) -> Option<embassy_usb::control::OutResponse> {
        use embassy_usb::control::{OutResponse, Recipient, RequestType};
        use odyssey_protocol::control::device::DeviceMsg;

        // Vendor|Interface, request SEND=0x30
        if req.request_type == RequestType::Vendor
            && req.recipient == Recipient::Interface
            && req.request == 0x30
        {
            match postcard::from_bytes::<DeviceMsg>(data) {
                Ok(msg) => {
                    (self.device_control_send)(msg);
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

        // Vendor|Interface, request RECV=0x31
        if req.request_type == RequestType::Vendor
            && req.recipient == Recipient::Interface
            && req.request == 0x31
        {
            if let Some(msg) = (self.device_control_recv)() {
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

/// Reads through a short packet, including a ZLP after a full-size payload.
/// An exact-fit payload still needs its terminating packet consumed; the
/// endpoint reports overflow if that packet contains more data instead.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn read_transfer<'d>(
    ep: &mut Endpoint<'d, Out>,
    buf: &mut [u8],
) -> Result<usize, EndpointError> {
    let max_packet = max(ep.info().max_packet_size, 1) as usize;
    let mut offset = 0;

    loop {
        let n = ep.read(&mut buf[offset..]).await?;
        offset += n;

        if n < max_packet {
            break;
        }
    }

    Ok(offset)
}

/// Since: Odyssey Desktop 0.7.0.
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
