#![no_std]
#![no_main]
#![expect(
    unsafe_code,
    reason = "Cortex-M exception entrypoints require unsafe signatures"
)]

mod hardware;

use core::cell::RefCell;

use better_dfu::consts::DfuAttributes;
use better_dfu::{ResetImmediate, application};
use embassy_boot::{AlignedBuffer, BlockingFirmwareUpdater, FirmwareUpdaterConfig};
use embassy_executor::Spawner;
use embassy_nrf::config::HfclkSource;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::pac;
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Duration, Timer};
use embassy_usb::{Builder, msos};
use embedded_storage::nor_flash::NorFlash;
use hardware::{DEVICE_INTERFACE_GUIDS, Irqs, Markers, force_markers_off, reset_safe};
use marker_control_board::PowerButton;

include!(concat!(env!("OUT_DIR"), "/version.rs"));
const FIRMWARE_BCD_DEVICE: u16 =
    odyssey_protocol::UsbDeviceRelease::from_version(FIRMWARE_VERSION).raw_value();

struct SafeDfuMarker<'d, DFU: NorFlash, STATE: NorFlash>(BlockingFirmwareUpdater<'d, DFU, STATE>);

impl<DFU: NorFlash, STATE: NorFlash> application::DfuMarker for SafeDfuMarker<'_, DFU, STATE> {
    fn mark_dfu(&mut self) {
        force_markers_off();
        if self.0.mark_dfu().is_err() {
            reset_safe();
        }
    }
}

fn sleep_until_button() -> ! {
    force_markers_off();
    pac::P0
        .pin_cnf(11)
        .modify(|w| w.set_sense(pac::gpio::vals::Sense::LOW));
    pac::POWER.systemoff().write(|w| w.set_systemoff(true));
    loop {
        cortex_m::asm::wfe();
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    force_markers_off();
    let button_wake = pac::POWER.resetreas().read().off();
    hardware::clear_reset_reason();
    let mut config = embassy_nrf::config::Config::default();
    config.hfclk_source = HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(config);
    let mut twi_buffer = [0; 2];
    let (mut markers, mut button, usbd, nvmc) = Markers::new(p, &mut twi_buffer);
    let hardware_ready = markers.configure_off().is_ok();

    let flash = Mutex::new(RefCell::new(Nvmc::new(nvmc)));
    let fw_config = FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
    let mut aligned = AlignedBuffer([0; 4]);
    let mut updater = BlockingFirmwareUpdater::new(fw_config, &mut aligned.0);
    // Confirm only after safe output and current-driver initialization succeeded.
    if hardware_ready && updater.mark_booted().is_err() {
        reset_safe();
    }

    let serial = hardware::serial_number();
    let Ok(serial) = core::str::from_utf8(&serial) else {
        reset_safe()
    };
    let mut config = embassy_usb::Config::new(0x1915, 0x5213);
    config.manufacturer = Some("Odyssey Arm");
    config.product = Some("Odyssey Marker Control Board");
    config.serial_number = Some(serial);
    config.device_release = FIRMWARE_BCD_DEVICE;
    config.max_packet_size_0 = 64;
    let driver = Driver::new(usbd, Irqs, HardwareVbusDetect::new(Irqs));
    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut msos_descriptor = [0; 256];
    let mut control_buf = [0; 64];
    let mut state = application::Control::new(
        SafeDfuMarker(updater),
        DfuAttributes::WILL_DETACH,
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
    builder.msos_descriptor(msos::windows_version::WIN8_1, 2);
    builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
    ));
    application::usb_dfu(
        &mut builder,
        &mut state,
        Duration::from_millis(1000),
        |func| {
            func.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
            func.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
                "DeviceInterfaceGUIDs",
                msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
            ));
        },
    );
    let mut usb = builder.build();
    let power = async {
        if !hardware_ready {
            core::future::pending::<()>().await;
        }
        let mut state = if button_wake {
            PowerButton::woken_by_button()
        } else {
            PowerButton::default()
        };
        loop {
            let is_low = button.is_low();
            Timer::after_millis(30).await;
            if button.is_low() != is_low {
                continue;
            }
            if let Some(on) = state.observe(is_low) {
                if markers.set(on).is_err() {
                    reset_safe();
                }
                if !on && !hardware::usb_powered() {
                    sleep_until_button();
                }
            }
            if is_low {
                button.wait_for_high().await;
            } else {
                button.wait_for_low().await;
            }
        }
    };
    embassy_futures::join::join(usb.run(), power).await;
    reset_safe();
}

#[cortex_m_rt::exception]
unsafe fn HardFault(_: &cortex_m_rt::ExceptionFrame) -> ! {
    reset_safe()
}
#[cortex_m_rt::exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    reset_safe()
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    reset_safe()
}
