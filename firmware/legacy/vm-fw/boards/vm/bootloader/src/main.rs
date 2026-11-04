#![no_std]
#![no_main]

use core::cell::RefCell;

use better_dfu::consts::DfuAttributes;
use better_dfu::{Control, NullDualBankWriter, NullForwarder, ResetImmediate, usb_dfu};
use cortex_m_rt::{entry, exception};
#[cfg(feature = "defmt")]
use defmt_rtt as _;
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_nrf::peripherals;
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_sync::blocking_mutex::Mutex;
use embassy_usb::{Builder, msos};

embassy_nrf::bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler;
});

const DEVICE_INTERFACE_GUIDS: &[&str] = &["{265744E6-40E8-41C4-AE17-F6F2EB187F1C}"];

/// This bootloader's release counter. Raise it whenever the image changes: the
/// host replaces a bootloader only when the published counter is higher.
macro_rules! release {
    () => {
        1
    };
}

const BOOTLOADER_BCD_DEVICE: u16 = odyssey_protocol::BootloaderRelease::bcd_device(release!());

#[entry]
fn main() -> ! {
    let p = embassy_nrf::init(Default::default());

    // Delay before touching flash: with a debugger/RTT attached, flash accessed
    // too early after boot hard-faults.
    for _ in 0..10000000 {
        cortex_m::asm::nop();
    }

    let flash = Nvmc::new(p.NVMC);
    let flash = Mutex::new(RefCell::new(flash));

    let config = embassy_boot::BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();

    let mut aligned_buf = embassy_boot::AlignedBuffer([0; PAGE_SIZE]);
    let mut boot = embassy_boot::BootLoader::new(config);
    let state = boot.prepare_boot(aligned_buf.as_mut()).unwrap();

    if state == embassy_boot::State::DfuDetach {
        let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
        // Own product id, so the host can tell bootloader from application
        // without opening the device; bcdDevice then carries this bootloader's
        // version. The application's id plus 0x10.
        let mut config = embassy_usb::Config::new(0x1915, 0x521F);
        config.manufacturer = Some("Odyssey Arm");
        config.product = Some("USB-DFU Bootloader");
        config.serial_number = Some("1235678");
        config.device_release = BOOTLOADER_BCD_DEVICE;

        let fw_config =
            embassy_boot::FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
        let mut buffer = embassy_boot::AlignedBuffer([0; 4]);
        let updater = embassy_boot::BlockingFirmwareUpdater::new(fw_config, &mut buffer.0[..]);

        let mut config_descriptor = [0; 256];
        let mut bos_descriptor = [0; 256];
        let mut msos_descriptor = [0; 256];
        let mut control_buf = [0; 4096];
        let mut state = Control::new(
            updater,
            DfuAttributes::CAN_DOWNLOAD | DfuAttributes::WILL_DETACH,
            ResetImmediate,
            NullForwarder,
            NullDualBankWriter,
            false, // active_bank_is_a; irrelevant for a single-bank layout
        );
        let mut builder = Builder::new(
            driver,
            config,
            &mut config_descriptor,
            &mut bos_descriptor,
            &mut msos_descriptor,
            &mut control_buf,
        );

        // MSOS headers make Windows assign the WinUSB driver automatically
        // instead of by hand with a tool like Zadig. They have to be at device
        // level, and for a composite device at function level too.
        builder.msos_descriptor(msos::windows_version::WIN8_1, 2);
        builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
        builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
            "DeviceInterfaceGUIDs",
            msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
        ));

        usb_dfu::<_, _, _, _, _, _, 4096>(&mut builder, &mut state, |func| {
            // Function-level headers matter only for a composite device.
            func.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
            func.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
                "DeviceInterfaceGUIDs",
                msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
            ));
        });

        let mut dev = builder.build();

        embassy_futures::block_on(dev.run());
    }

    unsafe {
        let mut p = cortex_m::Peripherals::steal();
        cortex_m::interrupt::disable();
        cortex_m::interrupt::enable();
        p.SCB.invalidate_icache();
        p.SCB.vtor.write(active_offset);
        cortex_m::asm::bootload(active_offset as *const u32)
    }
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
