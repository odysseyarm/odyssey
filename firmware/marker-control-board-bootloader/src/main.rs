#![no_std]
#![no_main]
#![expect(
    unsafe_code,
    reason = "Cortex-M vector validation and application handoff require raw memory access"
)]

#[path = "../../marker-control-board/src/hardware.rs"]
mod hardware;
#[path = "../../marker-control-board/src/valid_vectors.rs"]
mod valid_vectors;

use core::cell::RefCell;

use better_dfu::consts::DfuAttributes;
use better_dfu::{Control, NullDualBankWriter, NullForwarder, ResetImmediate, usb_dfu};
use cortex_m_rt::entry;
use embassy_boot::{
    AlignedBuffer, BlockingFirmwareUpdater, BootLoader, BootLoaderConfig, FirmwareUpdaterConfig,
    State,
};
use embassy_nrf::config::HfclkSource;
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_sync::blocking_mutex::Mutex;
use embassy_usb::{Builder, msos};
use hardware::{DEVICE_INTERFACE_GUIDS, Irqs, Markers, force_markers_off, reset_safe};

include!(concat!(env!("OUT_DIR"), "/layout.rs"));

const VERSION: [u32; 3] = odyssey_version::parse_cargo_version(env!("CARGO_PKG_VERSION"));
const BOOTLOADER_BCD_DEVICE: u16 = odyssey_version::firmware_bcd_device(VERSION[0]);
embassy_nrf::bind_interrupts!(struct VbusIrqs {
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler;
});

#[entry]
fn main() -> ! {
    force_markers_off();
    let mut config = embassy_nrf::config::Config::default();
    config.hfclk_source = HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(config);
    let mut twi_buffer = [0; 2];
    let mut markers = Markers::new(
        hardware::MarkerPins {
            output_1: p.P0_15,
            output_2: p.P1_09,
            output_3: p.P0_17,
            output_4: p.P0_20,
            twi: p.TWISPI0,
            scl: p.P0_10,
            sda: p.P0_09,
        },
        &mut twi_buffer,
    );
    let button = embassy_nrf::gpio::Input::new(p.P0_11, embassy_nrf::gpio::Pull::Up);
    let (usbd, nvmc) = (p.USBD, p.NVMC);
    let hardware_ready = markers.configure_off().is_ok();
    let held = button.is_low();
    cortex_m::asm::delay(1_280_000);
    let physical_dfu =
        held && button.is_low() && embassy_nrf::pac::POWER.usbregstatus().read().vbusdetect();

    let flash = Mutex::new(RefCell::new(Nvmc::new(nvmc)));
    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let mut boot = BootLoader::new(config);
    let mut aligned = AlignedBuffer([0; PAGE_SIZE]);
    // Finish interrupted swap/revert before considering a jump or physical DFU request.
    let prepared = boot.prepare_boot(aligned.as_mut());
    // SAFETY: memory map fixes this aligned vector address inside on-chip flash.
    let sp = unsafe { (ACTIVE_START as *const u32).read_volatile() };
    // SAFETY: second vector lies four bytes after the aligned active start.
    let reset = unsafe { ((ACTIVE_START + 4) as *const u32).read_volatile() };
    let vectors_valid =
        valid_vectors::valid_vectors(sp, reset, ACTIVE_START..ACTIVE_END, RAM_START..RAM_END);
    if hardware_ready
        && !physical_dfu
        && matches!(prepared, Ok(state) if state != State::DfuDetach)
        && vectors_valid
    {
        drop(markers);
        force_markers_off();
        // SAFETY: swap completed, hardware initialized off, and application vectors validated.
        unsafe { jump_to_app() }
    }
    hardware::clear_reset_reason();

    let fw_config = FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
    let mut state_buf = AlignedBuffer([0; 4]);
    let updater = BlockingFirmwareUpdater::new(fw_config, &mut state_buf.0);
    let serial = hardware::serial_number();
    let Ok(serial) = core::str::from_utf8(&serial) else {
        reset_safe()
    };
    let mut config = embassy_usb::Config::new(0x1915, 0x5223);
    config.manufacturer = Some("Odyssey Arm");
    config.product = Some("Odyssey Marker USB-DFU Bootloader");
    config.serial_number = Some(serial);
    config.device_release = BOOTLOADER_BCD_DEVICE;
    config.max_packet_size_0 = 64;
    let driver = Driver::new(usbd, Irqs, HardwareVbusDetect::new(VbusIrqs));
    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut msos_descriptor = [0; 512];
    let mut control_buf = [0; 4096];
    let mut state = Control::<_, _, _, NullForwarder, NullDualBankWriter, 4096>::new_single_bank(
        updater,
        DfuAttributes::CAN_DOWNLOAD | DfuAttributes::WILL_DETACH,
        ResetImmediate,
        (ACTIVE_END - ACTIVE_START) as usize,
    );
    state.set_firmware_version(env!("CARGO_PKG_VERSION"));
    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buf,
    );
    builder.msos_descriptor(msos::windows_version::WIN8_1, 2);
    builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
    ));
    usb_dfu::<_, _, _, _, _, _, 4096>(&mut builder, &mut state, |func| {
        func.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
        func.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
            "DeviceInterfaceGUIDs",
            msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
        ));
    });
    let mut usb = builder.build();
    embassy_futures::block_on(usb.run());
}

unsafe fn jump_to_app() -> ! {
    cortex_m::interrupt::disable();
    embassy_nrf::pac::RTC1.intenclr().write(|w| w.0 = u32::MAX);
    embassy_nrf::pac::RTC1.tasks_stop().write_value(1);
    // SAFETY: bootloader owns core peripherals; interrupts remain masked until all sources clear.
    let mut p = unsafe { cortex_m::Peripherals::steal() };
    p.SYST.disable_counter();
    p.SYST.disable_interrupt();
    for register in p.NVIC.icer.iter().take(2) {
        // SAFETY: bootloader owns NVIC; masking every external IRQ prevents stale handlers.
        unsafe { register.write(u32::MAX) };
    }
    for register in p.NVIC.icpr.iter().take(2) {
        // SAFETY: bootloader owns NVIC; clearing pending IRQs precedes application handoff.
        unsafe { register.write(u32::MAX) };
    }
    // SAFETY: active start is an aligned vector table validated before this call.
    unsafe { p.SCB.vtor.write(ACTIVE_START) };
    cortex_m::peripheral::SCB::clear_pendst();
    cortex_m::peripheral::SCB::clear_pendsv();
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
    // SAFETY: all sources are disabled and pending IRQs cleared with application VTOR installed.
    unsafe { cortex_m::interrupt::enable() };
    // SAFETY: stack and Thumb reset vectors point inside validated RAM and active flash.
    unsafe { cortex_m::asm::bootload(ACTIVE_START as *const u32) }
}

#[cortex_m_rt::exception]
unsafe fn HardFault(_: &cortex_m_rt::ExceptionFrame) -> ! {
    reset_safe()
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    reset_safe()
}

#[cortex_m_rt::exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    reset_safe()
}
