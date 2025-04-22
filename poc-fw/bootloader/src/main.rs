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
    //    for _ in 0..10000000 {
    //        cortex_m::asm::nop();
    //    }

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
        let mut config = embassy_usb::Config::new(0x1915, 0x5211);
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

        usb_dfu::<_, _, _, DfuPrinter, 4096>(&mut builder, &mut state);

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

// pub fn crc32_simple(mut data: &[u8]) -> u32 {
//     let mut crc = 0xFFFF_FFFFu32;
// 
//     while let Some((&byte, rest)) = data.split_first() {
//         let mut curr = byte as u32;
//         for _ in 0..8 {
//             let mix = (crc ^ curr) & 1;
//             crc >>= 1;
//             if mix != 0 {
//                 crc ^= 0xEDB88320;
//             }
//             curr >>= 1;
//         }
//         data = rest;
//     }
// 
//     !crc
// }

pub struct DfuPrinter;

unsafe extern "C" {
    static __bootloader_dfu_start: u8;
    static __bootloader_dfu_end: u8;
}

impl embassy_usb_dfu::Reset for DfuPrinter {
    fn sys_reset() -> ! {
        unsafe {
            let start = &__bootloader_dfu_start as *const u8;
            let end = &__bootloader_dfu_end as *const u8;
            let len = end.offset_from(start) as usize;

            let dfu_data: &[u8] = core::slice::from_raw_parts(start, len);

            // let crc = crc32_simple(dfu_data);
            // defmt::info!("DFU partition checksum (CRC32): 0x{:08x} ({} bytes)", crc, len);
        }

        loop {}
    }
}
