#![no_std]
#![no_main]
#![expect(
    unsafe_code,
    reason = "Cortex-M exception entrypoints require unsafe signatures"
)]

mod ble;
mod hardware;

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};

use better_dfu::consts::DfuAttributes;
use better_dfu::{Reset, application};
use embassy_boot::{
    AlignedBuffer, BlockingFirmwareUpdater, FirmwareUpdater, FirmwareUpdaterConfig,
};
use embassy_embedded_hal::flash::partition::Partition;
use embassy_executor::Spawner;
use embassy_nrf::config::{HfclkSource, LfclkSource};
use embassy_nrf::interrupt::Priority;
use embassy_nrf::interrupt::typelevel::Interrupt;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::signal::Signal;
use embassy_time::Duration;
use embassy_usb::{Builder, msos};
use hardware::{DEVICE_INTERFACE_GUIDS, Irqs, Markers, force_markers_off, reset_safe};
use nrf_sdc::{self as sdc, mpsl};
use static_cell::StaticCell;

const FIRMWARE_VERSION: [u32; 3] = odyssey_version::cargo_version!();
include!(concat!(env!("OUT_DIR"), "/owner_bonds.rs"));
const FIRMWARE_BCD_DEVICE: u16 = odyssey_version::firmware_bcd_device(FIRMWARE_VERSION[0]);

struct SafeDfuMarker;
struct DeferredDfuReset;
pub(crate) static DFU_PENDING: AtomicBool = AtomicBool::new(false);
static DFU_REQUEST: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub(crate) static OTA_ACTIVE: AtomicBool = AtomicBool::new(false);
pub(crate) static OTA_COMMITTING: AtomicBool = AtomicBool::new(false);
static DFU_DEFERRED: AtomicBool = AtomicBool::new(false);
pub(crate) static OTA_COMMIT_REQUEST: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub(crate) static OTA_COMMIT_RESULT: Signal<CriticalSectionRawMutex, bool> = Signal::new();
pub(crate) static OTA_RESET_REQUEST: Signal<CriticalSectionRawMutex, ()> = Signal::new();

impl Reset for DeferredDfuReset {
    fn sys_reset(&self) {
        DFU_REQUEST.signal(());
    }
}

impl application::DfuMarker for SafeDfuMarker {
    fn mark_dfu(&mut self) {
        DFU_PENDING.store(true, Ordering::Relaxed);
        force_markers_off();
        // Expander-driven current source and status LED only clear from the outputs task.
        ble::CHANGED.signal(());
    }
}

embassy_nrf::bind_interrupts!(struct RadioIrqs {
    CLOCK_POWER => mpsl::ClockInterruptHandler, embassy_nrf::usb::vbus_detect::InterruptHandler;
    EGU0_SWI0 => mpsl::LowPrioInterruptHandler;
    RADIO => mpsl::HighPrioInterruptHandler;
    TIMER0 => mpsl::HighPrioInterruptHandler;
    RTC0 => mpsl::HighPrioInterruptHandler;
});

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static mpsl::MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    force_markers_off();
    hardware::clear_reset_reason();
    let mut config = embassy_nrf::config::Config::default();
    config.hfclk_source = HfclkSource::ExternalXtal;
    // The BC833M module carries a 20 ppm 32.768 kHz crystal and its load capacitors.
    config.lfclk_source = LfclkSource::ExternalXtal;
    config.gpiote_interrupt_priority = Priority::P6;
    config.time_interrupt_priority = Priority::P6;
    let p = embassy_nrf::init(config);
    embassy_nrf::interrupt::typelevel::USBD::set_priority(Priority::P6);
    embassy_nrf::interrupt::typelevel::TWISPI0::set_priority(Priority::P6);
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
    let (usbd, mut nvmc) = (p.USBD, p.NVMC);
    // A single NAK here would leave the image unconfirmed and the radio parked,
    // so the bus gets a few attempts before the board gives up on it.
    let hardware_ready = (0..3).any(|_| markers.configure_off().is_ok());

    {
        let flash = Mutex::new(RefCell::new(Nvmc::new(nvmc.reborrow())));
        let fw_config = FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
        let mut aligned = AlignedBuffer([0; 4]);
        let mut updater = BlockingFirmwareUpdater::new(fw_config, &mut aligned.0);
        // Confirm before MPSL starts and only after safe current-driver initialization.
        if hardware_ready && updater.mark_booted().is_err() {
            reset_safe();
        }
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
    let driver = Driver::new(usbd, Irqs, HardwareVbusDetect::new(RadioIrqs));
    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut msos_descriptor = [0; 512];
    let mut control_buf = [0; 64];
    let mut state =
        application::Control::new(SafeDfuMarker, DfuAttributes::WILL_DETACH, DeferredDfuReset);
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
    let automatic = async {
        let mpsl_p =
            mpsl::Peripherals::new(p.RTC0, p.TIMER0, p.TEMP, p.PPI_CH19, p.PPI_CH30, p.PPI_CH31);
        let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
            source: mpsl::raw::MPSL_CLOCK_LF_SRC_XTAL as u8,
            rc_ctiv: 0,
            rc_temp_ctiv: 0,
            // Crystal's 20 ppm plus temperature and ageing margin.
            // Matches the nRF Connect SDK's 50 ppm default.
            accuracy_ppm: 50,
            skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
        };
        static MPSL: StaticCell<mpsl::MultiprotocolServiceLayer> = StaticCell::new();
        static TIMESLOTS: StaticCell<mpsl::SessionMem<1>> = StaticCell::new();
        let Ok(service) = mpsl::MultiprotocolServiceLayer::with_timeslots(
            mpsl_p,
            RadioIrqs,
            lfclk_cfg,
            TIMESLOTS.init(mpsl::SessionMem::new()),
        ) else {
            reset_safe();
        };
        let mpsl = MPSL.init(service);
        let Ok(task) = mpsl_task(mpsl) else {
            reset_safe();
        };
        spawner.spawn(task);
        let flash = AsyncMutex::<NoopRawMutex, _>::new(mpsl::Flash::take(mpsl, nvmc));
        let fw_config = FirmwareUpdaterConfig::from_linkerfile(&flash, &flash);
        let mut aligned = AlignedBuffer([0; 4]);
        let updater = AsyncMutex::<NoopRawMutex, _>::new(FirmwareUpdater::new(fw_config, &mut aligned.0));
        let radio = async {
            if !hardware_ready {
                core::future::pending::<()>().await;
            }
            let sdc_p = sdc::Peripherals::new(
                p.PPI_CH17, p.PPI_CH18, p.PPI_CH20, p.PPI_CH21, p.PPI_CH22, p.PPI_CH23, p.PPI_CH24,
                p.PPI_CH25, p.PPI_CH26, p.PPI_CH27, p.PPI_CH28, p.PPI_CH29,
            );
            let mut rng = embassy_nrf::rng::Rng::new_blocking(p.RNG);
            let mut seed = [0; 32];
            rng.blocking_fill_bytes(&mut seed);
            let mut mem = sdc::Mem::<8192>::new();
            let Ok(builder) = sdc::Builder::new()
                .and_then(|builder| {
                    builder
                        .support_adv()
                        .support_peripheral()
                        .peripheral_count(1)
                })
                .and_then(|builder| builder.buffer_cfg(27, 27, 3, 3))
            else {
                reset_safe();
            };
            let Ok(controller) = builder.build(sdc_p, &mut rng, mpsl, &mut mem) else {
                reset_safe();
            };
            embassy_futures::join::join3(
                ble::run(
                    controller,
                    Partition::new(&flash, OWNER_BONDS_START, 8192),
                    &flash,
                    seed,
                ),
                ble::outputs(markers),
                ble::button(button),
            )
            .await;
        };
        let detach = async {
            loop {
                match embassy_futures::select::select(
                    DFU_REQUEST.wait(),
                    OTA_COMMIT_REQUEST.wait(),
                )
                .await
                {
                    embassy_futures::select::Either::First(()) => {
                        if OTA_COMMITTING.load(Ordering::Acquire) {
                            DFU_DEFERRED.store(true, Ordering::Release);
                            continue;
                        }
                        DFU_PENDING.store(true, Ordering::Release);
                        force_markers_off();
                        ble::CHANGED.signal(());
                        if updater.lock().await.mark_dfu().await.is_err() {
                            reset_safe();
                        }
                        reset_safe();
                    }
                    embassy_futures::select::Either::Second(()) => {
                        OTA_COMMITTING.store(true, Ordering::Release);
                        DFU_PENDING.store(true, Ordering::Release);
                        force_markers_off();
                        ble::CHANGED.signal(());
                        let committed = updater.lock().await.mark_updated().await.is_ok();
                        if !committed {
                            OTA_COMMITTING.store(false, Ordering::Release);
                            DFU_PENDING.store(false, Ordering::Release);
                            if DFU_DEFERRED.swap(false, Ordering::AcqRel) {
                                DFU_REQUEST.signal(());
                            }
                        }
                        OTA_COMMIT_RESULT.signal(committed);
                        if committed {
                            OTA_RESET_REQUEST.wait().await;
                            reset_safe();
                        }
                    }
                }
            }
        };
        embassy_futures::join::join(radio, detach).await;
    };
    embassy_futures::join::join(usb.run(), automatic).await;
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
