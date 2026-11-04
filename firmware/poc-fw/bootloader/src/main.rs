#![no_std]
#![no_main]

use core::cell::RefCell;

use cortex_m_rt::{entry, exception};
#[cfg(feature = "defmt")]
use defmt_rtt as _;
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_nrf::peripherals;
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_sync::blocking_mutex::Mutex;
use embassy_usb::{Builder, msos};
use embassy_usb_dfu::consts::DfuAttributes;
use embassy_usb_dfu::{Control, ResetImmediate, usb_dfu};

embassy_nrf::bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
});

#[entry]
fn main() -> ! {
    let p = embassy_nrf::init(Default::default());

    // With a debugger or RTT attached, touching flash this soon after boot can hard fault.
    // Enable this delay when debugging the bootloader.
    // for _ in 0..10000000 {
    //     cortex_m::asm::nop();
    // }

    let flash = Nvmc::new(p.NVMC);
    let flash = Mutex::new(RefCell::new(flash));

    let config = embassy_boot::BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();

    let mut aligned_buf = embassy_boot::AlignedBuffer([0; PAGE_SIZE]);
    let mut boot = embassy_boot::BootLoader::new(config);
    let state = boot.prepare_boot(aligned_buf.as_mut()).unwrap();

    if state == embassy_boot::State::DfuDetach {
        let driver = Driver::new(p.USBD, Irqs, HardwareVbusDetect::new(Irqs));
        let mut config = embassy_usb::Config::new(0x1915, 0x5211);
        config.manufacturer = Some("Nordic");
        config.product = Some("USB-DFU Bootloader");
        config.serial_number = Some("1235678");

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
        );
        let mut builder = Builder::new(
            driver,
            config,
            &mut config_descriptor,
            &mut bos_descriptor,
            &mut msos_descriptor,
            &mut control_buf,
        );

        // MSOS descriptors make Windows bind the WinUSB driver automatically; without them users
        // have to install it by hand with a tool such as Zadig. They are needed at the device
        // level, and composite devices also need them at the function level (see below).
        builder.msos_descriptor(msos::windows_version::WIN8_1, 2);
        builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));

        usb_dfu::<_, _, _, _, 4096>(&mut builder, &mut state, |func| {
            // Function-level headers are only needed for a composite device, i.e. one that
            // exposes another interface besides DFU.
            // func.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
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
    unsafe {
        const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
        let irqn = core::ptr::read_volatile(SCB_ICSR) as u8 as i16 - 16;

        panic!("DefaultHandler #{:?}", irqn);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    cortex_m::asm::udf();
}
