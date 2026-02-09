//! Minimal USB enumeration test for lite1
//!
//! This firmware only initializes USB to verify the USB hardware works.
//! If this enumerates on the host, USB is working and the issue is
//! with other peripheral initialization.

#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;
use embassy_nrf::{bind_interrupts, peripherals, usb};
use embassy_time::Timer;
use embassy_usb::msos;

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => usb::vbus_detect::InterruptHandler;
});

const DEVICE_INTERFACE_GUID: &str = "{A4769731-EC56-49FF-9924-613E5B3D4D6C}";

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    defmt::info!("USB test starting...");

    // Minimal init - just enough for USB
    let mut config = embassy_nrf::config::Config::default();
    config.hfclk_source = embassy_nrf::config::HfclkSource::ExternalXtal;
    config.lfclk_source = embassy_nrf::config::LfclkSource::ExternalXtal;
    config.dcdc.regh = true;

    let p = embassy_nrf::init(config);
    defmt::info!("Embassy init complete");

    // Create USB driver
    let vbus = usb::vbus_detect::HardwareVbusDetect::new(Irqs);
    let driver = usb::Driver::new(p.USBD, Irqs, vbus);

    // USB config - match the common::usb setup exactly
    let mut usb_config = embassy_usb::Config::new(0x1915, 0x5211);
    usb_config.manufacturer = Some("Odyssey Arm");
    usb_config.product = Some("ATS USB Legacy");
    usb_config.max_power = 310;
    usb_config.max_packet_size_0 = 64;
    // Composite device settings (needed for MSOS)
    usb_config.composite_with_iads = true;
    usb_config.device_class = 0xEF;
    usb_config.device_sub_class = 0x02;
    usb_config.device_protocol = 0x01;

    // Allocate buffers
    static CONFIG_DESC: static_cell::StaticCell<[u8; 256]> = static_cell::StaticCell::new();
    static BOS_DESC: static_cell::StaticCell<[u8; 256]> = static_cell::StaticCell::new();
    static MSOS_DESC: static_cell::StaticCell<[u8; 512]> = static_cell::StaticCell::new();
    static CONTROL_BUF: static_cell::StaticCell<[u8; 64]> = static_cell::StaticCell::new();

    let config_desc = CONFIG_DESC.init([0u8; 256]);
    let bos_desc = BOS_DESC.init([0u8; 256]);
    let msos_desc = MSOS_DESC.init([0u8; 512]);
    let control_buf = CONTROL_BUF.init([0u8; 64]);

    let mut builder = embassy_usb::Builder::new(
        driver,
        usb_config,
        config_desc,
        bos_desc,
        msos_desc,
        control_buf,
    );

    // Add MSOS descriptor for Windows (this is critical for WinUSB)
    builder.msos_descriptor(msos::windows_version::WIN8_1, 0);

    // Add a vendor-specific function with WINUSB compatible ID
    let mut function = builder.function(0xFF, 0, 0);
    function.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    function.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(&[DEVICE_INTERFACE_GUID]),
    ));
    let mut interface = function.interface();
    let mut alt = interface.alt_setting(0xFF, 0, 0, None);
    let _ep_out = alt.endpoint_bulk_out(None, 64);
    let _ep_in = alt.endpoint_bulk_in(None, 64);
    drop(function);

    let mut usb = builder.build();
    defmt::info!("USB device built, starting...");

    spawner.spawn(blink_task().unwrap());

    // Run USB forever
    usb.run().await;
}

#[embassy_executor::task]
async fn blink_task() {
    loop {
        defmt::info!("USB running...");
        Timer::after_secs(5).await;
    }
}
