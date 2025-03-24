#![no_std]
#![no_main]

use core::cell::RefCell;

use cortex_m_rt::{entry, exception};
#[cfg(feature = "defmt")]
use defmt_rtt as _;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::nvmc::PAGE_SIZE;
use embassy_nrf::{
    peripherals::self,
    usb::{vbus_detect::HardwareVbusDetect, Driver},
};
use embassy_sync::blocking_mutex::Mutex;
use embassy_usb::{msos, Builder};
use embassy_usb_dfu::consts::DfuAttributes;
use embassy_usb_dfu::{usb_dfu, Control, ResetImmediate};

embassy_nrf::bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
});

// This is a randomly generated GUID to allow clients on Windows to find our device
const DEVICE_INTERFACE_GUIDS: &[&str] = &["{EAA9A5DC-30BA-44BC-9232-606CDC875321}"];

#[entry]
fn main() -> ! {
    let p = embassy_nrf::init(Default::default());

    // Uncomment this if you are debugging the bootloader with debugger/RTT attached,
    // as it prevents a hard fault when accessing flash 'too early' after boot.
    /*
        for i in 0..10000000 {
            cortex_m::asm::nop();
        }
    */

    let flash = Nvmc::new(p.NVMC);
    let flash = Mutex::new(RefCell::new(flash));

    let config = embassy_boot::BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();

    let mut aligned_buf = embassy_boot::AlignedBuffer([0; PAGE_SIZE]);
    let mut boot = embassy_boot::BootLoader::new(config);
    let state = boot.prepare_boot(aligned_buf.as_mut()).unwrap();

    if state == embassy_boot::State::DfuDetach {
        // Create the driver, from the HAL.
        let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
        let mut config = embassy_usb::Config::new(0xc0de, 0xcafe);
        config.manufacturer = Some("Nordic");
        config.product = Some("USB-DFU Bootloader");
        config.serial_number = Some("1235678");

        let fw_config =
            embassy_boot::FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
        let mut buffer = embassy_boot::AlignedBuffer([0; 4usize]);
        let updater = embassy_boot::BlockingFirmwareUpdater::new(fw_config, &mut buffer.0[..]);

        let mut config_descriptor = [0; 256];
        let mut bos_descriptor = [0; 256];
        let mut control_buf = [0; 4096];
        let mut state = Control::new(updater, DfuAttributes::CAN_DOWNLOAD);
        let mut builder = Builder::new(
            driver,
            config,
            &mut config_descriptor,
            &mut bos_descriptor,
            &mut [],
            &mut control_buf,
        );

        // We add MSOS headers so that the device automatically gets assigned the WinUSB driver on Windows.
        // Otherwise users need to do this manually using a tool like Zadig.
        //
        // It seems it is important for the DFU class that these headers be on the Device level.
        //
        builder.msos_descriptor(msos::windows_version::WIN8_1, 2);
        builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
        builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
            "DeviceInterfaceGUIDs",
            msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
        ));

        usb_dfu::<_, _, _, ResetImmediate, 4096>(&mut builder, &mut state);

        let mut dev = builder.build();
        embassy_futures::block_on(dev.run());
    }

    unsafe {
        let mut p = cortex_m::Peripherals::steal();
        p.SCB.invalidate_icache();
        p.SCB.vtor.write(active_offset);
        cortex_m::asm::bootload(active_offset as *const u32)
    }
}

#[no_mangle]
#[cfg_attr(target_os = "none", link_section = ".HardFault.user")]
unsafe extern "C" fn HardFault() {
    cortex_m::peripheral::SCB::sys_reset();
}

#[exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
    let irqn = core::ptr::read_volatile(SCB_ICSR) as u8 as i16 - 16;

    panic!("DefaultHandler #{:?}", irqn);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    cortex_m::asm::udf();
}
