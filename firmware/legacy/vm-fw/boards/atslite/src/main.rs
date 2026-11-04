#![no_main]
#![no_std]

use core::cell::RefCell;

use atslite_board::l2cap::AtsliteL2capChannels;
use atslite_board::pins::Board;
use atslite_board::platform::AtslitePlatform;
use atslite_board::pmic_leds::{LedIdx, LedState, PmicLedAnimator, PmicLedsHandle};
use atslite_board::{
    Irqs, ble, embassy_executor, embassy_nrf, imu, nrf5340_init, power, power_button, power_state,
    split_board, utils,
};
use better_dfu::application::DfuMarker;
use better_dfu::consts::DfuAttributes;
use better_dfu::{Control, ResetImmediate, usb_dfu};
use common::object_mode::{ObjectModeContext, object_mode};
use common::odyssey_protocol::control::device::TransportMode;
use common::{Paj, settings, usb};
use defmt_rtt as _;
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{self, Input, Pin};
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::twim::{self, Twim};
use embassy_nrf::{pac, peripherals};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::{Delay, Timer};
use embassy_usb::msos;
use npm1300_rs::{self, NPM1300};
use panic_probe as _;
use static_cell::StaticCell;

const DEVICE_INTERFACE_GUID: &str = "{A4769731-EC56-49FF-9924-613E5B3D4D6C}";
const DFU_INTERFACE_GUID: &str = "{72DC6483-1013-4BC3-B1CF-6A02DDAEFCE5}";

const fn parse_version_component(s: &str) -> u16 {
    let b = s.as_bytes();
    let mut val: u16 = 0;
    let mut i = 0;
    while i < b.len() {
        val = val * 10 + (b[i] - b'0') as u16;
        i += 1;
    }
    val
}

const FIRMWARE_VERSION: [u16; 3] = [
    parse_version_component(env!("CARGO_PKG_VERSION_MAJOR")),
    parse_version_component(env!("CARGO_PKG_VERSION_MINOR")),
    parse_version_component(env!("CARGO_PKG_VERSION_PATCH")),
];

unsafe extern "C" {
    static __bootloader_state_start: u32;
}

#[repr(C)]
struct BootStateRaw {
    magic: u32,
    active_slot: u8,
    pending_slot: u8,
    boot_counter: u8,
    _reserved: u8,
}

fn log_boot_state(label: &str) {
    unsafe {
        let state_addr = &__bootloader_state_start as *const u32 as usize;
        defmt::info!("[bootlog {}] state_addr={:#010x}", label, state_addr);
        let flag = core::ptr::read_volatile(state_addr as *const u32);
        let state_word =
            core::ptr::read_volatile((state_addr + core::mem::size_of::<u32>()) as *const u32);
        let state_struct = core::ptr::read_volatile(
            (state_addr + core::mem::size_of::<u32>()) as *const BootStateRaw,
        );
        defmt::info!(
            "[bootlog {}] flag={:#010x}, state_word={:#010x}, active={}, pending={}, attempts={}",
            label,
            flag,
            state_word,
            state_struct.active_slot,
            state_struct.pending_slot,
            state_struct.boot_counter
        );
    }
}

// DFU runtime detach writes this marker so the bootloader enters DFU on the next reset.
struct AppDfuMarker<'a> {
    flash: &'a Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>,
}

impl<'a> DfuMarker for AppDfuMarker<'a> {
    fn mark_dfu(&mut self) {
        defmt::info!("Dfu Detach");
        log_boot_state("before DFU request");

        let result = self.flash.lock(|flash_cell| {
            let mut flash = flash_cell.borrow_mut();
            atslite_boot_api::BootConfirmation::request_dfu_with(&mut flash)
        });

        match result {
            Ok(()) => {
                defmt::info!("[dfu_runtime] flagged DFU requested - SUCCESS");
                log_boot_state("after DFU request");
            }
            Err(()) => {
                defmt::warn!("[dfu_runtime] flagged DFU requested - FAILED");
                log_boot_state("after DFU request FAILED");
            }
        }
    }
}

/// Copies the general settings from `common::Settings` into atslite-common and
/// persists them through atslite-common's flash node. Registered as the
/// GENERAL_WRITE_FN, so a bulk FlashSettings over BLE ends up here.
fn sync_and_write_general() {
    let common_s = unsafe { common::settings::get_settings() };
    let atslite_s = unsafe { atslite_common::settings::get_settings() };
    atslite_s.general.impact_threshold = common_s.general.impact_threshold;
    atslite_s.general.suppress_ms = common_s.general.suppress_ms;
    atslite_s.general.accel_config = common_s.general.accel_config.clone();
    atslite_s.general.gyro_config = common_s.general.gyro_config.clone();
    atslite_s.general.camera_model_nf = common_s.general.camera_model_nf.clone();
    atslite_s.general.camera_model_wf = common_s.general.camera_model_wf.clone();
    atslite_s.general.stereo_iso = common_s.general.stereo_iso.clone();
    atslite_s.general.name = common_s.general.name.clone();
    // transport_mode is not copied: the device ignores it and picks its
    // transport from the USB state.
    atslite_s.general_write();
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    defmt::info!("ATSlite board initialization starting...");

    defmt::info!("Calling nrf5340_init::init()...");
    let (_cp, p) = nrf5340_init::init();
    defmt::info!("nrf5340_init complete");

    // NVMC is shared between DFU and settings.
    defmt::info!("Initializing NVMC...");
    static NVMC: StaticCell<Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>> = StaticCell::new();
    let nvmc = NVMC.init_with(|| Mutex::new(RefCell::new(Nvmc::new(p.NVMC))));
    defmt::info!("NVMC initialized");

    defmt::info!("Splitting board peripherals...");
    let board: Board = split_board!(p);
    defmt::info!("Board peripherals split");

    // Boot is confirmed after IMU init, further down.

    defmt::info!("Initializing PMIC...");
    let mut twim_cfg = twim::Config::default();
    twim_cfg.frequency = twim::Frequency::K400;
    static TWIM_BUF: StaticCell<[u8; 0]> = StaticCell::new();
    let twim_buf = TWIM_BUF.init([0u8; 0]);
    defmt::info!("Creating TWIM...");
    let twim = Twim::new(
        p.SERIAL2,
        Irqs,
        board.pmic.sda,
        board.pmic.scl,
        twim_cfg,
        twim_buf,
    );
    defmt::info!("TWIM created");

    static PMIC: StaticCell<AsyncMutex<NoopRawMutex, NPM1300<Twim<'static>, Delay>>> =
        StaticCell::new();
    defmt::info!("Creating NPM1300...");
    let mut pmic_dev = NPM1300::new(twim, Delay);
    defmt::info!("Running pmic_setup...");
    power::pmic_setup(&mut pmic_dev).await.unwrap();
    defmt::info!("Running configure_and_start_charging...");
    power::configure_and_start_charging(
        &mut pmic_dev,
        npm1300_rs::sysreg::VbusInCurrentLimit::MA100,
    )
    .await
    .unwrap();
    defmt::info!("PMIC configured");
    let pmic = PMIC.init(AsyncMutex::new(pmic_dev));

    defmt::info!("Spawning pmic_irq_task...");
    spawner.spawn(pmic_irq_task(board.pmic.irq.into(), pmic).unwrap());

    defmt::info!("Creating PmicLedAnimator...");
    let (anim, handle) = PmicLedAnimator::new(pmic, LedIdx::Ch0, LedIdx::Ch1, LedIdx::Ch2)
        .await
        .unwrap();
    defmt::info!("Spawning led_task...");
    spawner.spawn(led_task(anim).unwrap());
    static PMIC_LEDS_HANDLE: StaticCell<PmicLedsHandle> = StaticCell::new();
    let pmic_leds = PMIC_LEDS_HANDLE.init(handle);

    // The stage2 bootloader already started the watchdog.
    use embassy_nrf::wdt::{Config as WdtConfig, HaltConfig, SleepConfig, Watchdog};
    let mut wdt_config = WdtConfig::default();
    wdt_config.timeout_ticks = 32768 * 60;
    wdt_config.action_during_sleep = SleepConfig::RUN;
    wdt_config.action_during_debug_halt = HaltConfig::PAUSE;
    let (_wdt, [wdt_handle]) = Watchdog::try_new(p.WDT0, wdt_config).expect("Watchdog init failed");
    spawner.spawn(watchdog_feeder(wdt_handle).unwrap());
    let l2cap_channels = AtsliteL2capChannels::new();

    defmt::info!("Power button handling...");
    let mut pwr_btn = Input::new(board.pmic.pwr_btn, gpio::Pull::Up);
    defmt::info!("handle_boot_vbus_policy...");
    handle_boot_vbus_policy(pmic, pmic_leds).await.unwrap();
    defmt::info!("handle_pending_ship...");
    handle_pending_ship(pmic, pmic_leds, &mut pwr_btn)
        .await
        .unwrap();
    defmt::info!("Setting LED state TurningOn...");
    pmic_leds.set_state(LedState::TurningOn).await;
    let pwr_btn_task = pwr_btn;
    defmt::info!("Spawning power_button_task...");
    spawner.spawn(power_button_task(pwr_btn_task, pmic, pmic_leds).unwrap());
    defmt::info!("Power tasks spawned");

    defmt::info!("Initializing PAJ sensors...");
    let mut paj_cfg = embassy_nrf::spim::Config::default();
    paj_cfg.frequency = embassy_nrf::spim::Frequency::M8;
    paj_cfg.mode = embassy_nrf::spim::MODE_3;
    paj_cfg.bit_order = embassy_nrf::spim::BitOrder::LSB_FIRST;

    // SPI pins built by hand so `fod` stays available for object mode.
    let paj7025r2_spi = common::utils::SpiPins {
        sck: board.paj7025r2.sck.into(),
        miso: board.paj7025r2.miso.into(),
        mosi: board.paj7025r2.mosi.into(),
        cs: board.paj7025r2.cs.into(),
    };
    let paj7025r3_spi = common::utils::SpiPins {
        sck: board.paj7025r3.sck.into(),
        miso: board.paj7025r3.miso.into(),
        mosi: board.paj7025r3.mosi.into(),
        cs: board.paj7025r3.cs.into(),
    };

    defmt::info!("Creating SPI devices...");
    let dev_r2 = common::utils::make_spi_dev(p.SERIAL0, Irqs, paj7025r2_spi, paj_cfg.clone());
    defmt::info!("SPI dev_r2 created");
    let dev_r3 = common::utils::make_spi_dev(p.SERIAL1, Irqs, paj7025r3_spi, paj_cfg);
    defmt::info!("SPI dev_r3 created");

    defmt::info!("Waiting 1ms...");
    Timer::after_millis(1).await;
    defmt::info!("Timer done");

    defmt::info!("Initializing PAJ7025R2...");
    let mut paj7025r2 = Paj::init(dev_r2).await.unwrap();
    defmt::info!("Initializing PAJ7025R3...");
    let mut paj7025r3 = Paj::init(dev_r3).await.unwrap();
    defmt::info!("Configuring PAJ sensors...");
    paj7025r2.init_settings(true, 0x10, 0x00).await.unwrap();
    paj7025r3.init_settings(true, 0x10, 0x02).await.unwrap();
    defmt::info!("PAJ sensors configured");

    static PAJ7025R2_MUTEX: StaticCell<AsyncMutex<NoopRawMutex, Paj>> = StaticCell::new();
    static PAJ7025R3_MUTEX: StaticCell<AsyncMutex<NoopRawMutex, Paj>> = StaticCell::new();
    let paj_r2_mutex = PAJ7025R2_MUTEX.init(AsyncMutex::new(paj7025r2));
    let paj_r3_mutex = PAJ7025R3_MUTEX.init(AsyncMutex::new(paj7025r3));

    // ICM42688 IMU.
    defmt::info!("Initializing IMU...");
    unsafe {
        // Erratum workaround
        (0x5000_ac04 as *mut u32).write_volatile(1);
    }
    let (imu, imu_int) = imu::init(
        p.SPIM4,
        board.icm42688v.cs.into(),
        board.icm42688v.sck.into(),
        board.icm42688v.miso.into(),
        board.icm42688v.mosi.into(),
        board.icm42688v.int1.into(),
        board.icm42688v.clkin.into(),
        p.TIMER1, // TIMER0 is reserved for object mode
        p.PPI_CH0.into(),
        p.GPIOTE_CH0,
        common::settings::accel_odr(),
    )
    .await;
    defmt::info!("IMU initialized");

    // boot-api's mark_booted also caches the boot state for later DFU requests.
    log_boot_state("before mark_booted");

    nvmc.lock(|flash_cell| {
        let mut flash = flash_cell.borrow_mut();
        atslite_boot_api::BootConfirmation::mark_booted(&mut *flash).expect("Failed to mark boot");
    });

    log_boot_state("after mark_booted");

    // Device control channels exist before USB, because the host sends control
    // requests right after enumeration. The same channels are registered in
    // common (USB handler) and atslite_common (device_control_task) so both
    // share one set of queues.
    let (ctrl_evt_ch, ctrl_cmd_ch) = common::device_control::init();
    common::device_control::register(ctrl_evt_ch, ctrl_cmd_ch);
    atslite_common::device_control::register(ctrl_evt_ch, ctrl_cmd_ch);

    static DFU_STATE: StaticCell<Control<AppDfuMarker<'static>, ResetImmediate>> =
        StaticCell::new();
    let dfu_state = DFU_STATE.init(Control::new(
        AppDfuMarker { flash: nvmc },
        DfuAttributes::CAN_DOWNLOAD | DfuAttributes::WILL_DETACH,
        ResetImmediate,
    ));

    let vbus = embassy_nrf::usb::vbus_detect::HardwareVbusDetect::new(Irqs);
    let (usb_dev, usb_snd, usb_rcv, usb_cfg) = usb::usb_device(
        0x5210, // Atslite PID
        FIRMWARE_VERSION,
        DEVICE_INTERFACE_GUID,
        p.USBD,
        Irqs,
        vbus,
        |builder| {
            // DFU runtime interface, for detach.
            usb_dfu(
                builder,
                dfu_state,
                embassy_time::Duration::from_millis(2500),
                |func| {
                    func.msos_feature(embassy_usb::msos::CompatibleIdFeatureDescriptor::new(
                        "WINUSB", "",
                    ));
                    func.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
                        "DeviceInterfaceGUIDs",
                        msos::PropertyData::RegMultiSz(&[DFU_INTERFACE_GUID]),
                    ));
                },
            );
        },
    );
    spawner.spawn(usb::run_usb(usb_dev).unwrap());

    // Flash writes on the nRF5340 keep running in hardware after the NVMC call
    // returns, and re-creating NVMC while the controller is busy corrupts the
    // latest write. 200 ms covers the boot confirmation write at 0xD000.
    Timer::after_millis(200).await;

    let nvmc_async = nvmc.lock(|_f| {
        // Stealing NVMC is sound here: nothing else writes flash in normal
        // operation, and a DFU detach resets the device.
        BlockingAsync::new(unsafe { Nvmc::new(peripherals::NVMC::steal()) })
    });
    // Settings come up in phases. First the shared settings worker owned by atslite-common.
    use atslite_common::settings as shared_settings;
    shared_settings::init_core(&spawner, nvmc_async).await;

    // Then the legacy board-specific nodes, on the shared LIST.
    let settings_ref = {
        let mut r2 = paj_r2_mutex.lock().await;
        let mut r3 = paj_r3_mutex.lock().await;
        settings::init_legacy_settings(
            &shared_settings::LIST,
            shared_settings::settings_flush_now,
            &mut *r2,
            &mut *r3,
        )
        .await
        .unwrap()
    };

    // BLE bond node; also sets atslite-common's SETTINGS_PTR.
    let atslite_settings = shared_settings::init_nodes().await;

    // General settings node (atslite-common schema), loaded from flash. This
    // makes atslite_common own "settings/general", the key and schema that
    // device_control_task uses for FlashSettings, SetDeviceName, WriteConfig.
    shared_settings::init_board_nodes(atslite_settings).await;

    // Copy general settings into legacy common settings, which object_mode
    // reads through its &mut common::Settings.
    {
        let ag = &atslite_settings.general;
        settings_ref.general.impact_threshold = ag.impact_threshold;
        settings_ref.general.suppress_ms = ag.suppress_ms;
        settings_ref.general.accel_config = ag.accel_config.clone();
        settings_ref.general.gyro_config = ag.gyro_config.clone();
        settings_ref.general.camera_model_nf = ag.camera_model_nf.clone();
        settings_ref.general.camera_model_wf = ag.camera_model_wf.clone();
        settings_ref.general.stereo_iso = ag.stereo_iso.clone();
        settings_ref.general.name = ag.name.clone();
    }
    settings::set_accel_odr(settings_ref.general.accel_config.accel_odr);

    // Round accel_odr to the nearest rate this sensor supports.
    settings_ref.general.accel_config.accel_odr =
        imu::round_accel_odr_hz(settings_ref.general.accel_config.accel_odr);

    // Boot state again, to catch corruption from settings init.
    log_boot_state("after settings init");
    defmt::info!("Settings initialization complete");

    // A BLE FlashSettings (bulk PacketData::FlashSettings ->
    // common::Settings::write()) persists general settings through
    // atslite-common's flash node.
    settings::register_general_write_fn(sync_and_write_general);

    // Its channels were registered before USB init.
    spawner.spawn(
        atslite_board::device_control_task::device_control_task(FIRMWARE_VERSION, active_transport)
            .unwrap(),
    );

    // Raise the PMIC current limit once USB enumerates.
    spawner.spawn(usb_pmic_config_task(usb_cfg, pmic).unwrap());

    // Seed for trouble-host's PRNG. CryptoCell has to be disabled afterwards or
    // the CPU cannot sleep.
    defmt::info!("Generating BLE seed from CryptoCell...");
    let ble_seed = {
        use embassy_nrf::cryptocell_rng::CcRng;
        let mut seed = [0u8; 32];
        defmt::info!("Creating CcRng...");
        let mut cc_rng = CcRng::new_blocking(p.CC_RNG);
        defmt::info!("Filling seed bytes...");
        cc_rng.blocking_fill_bytes(&mut seed);
        defmt::info!("Dropping CcRng...");
        drop(cc_rng);
        defmt::info!("Disabling CryptoCell...");
        embassy_nrf::pac::CRYPTOCELL
            .enable()
            .write(|w| w.set_enable(false));
        defmt::info!("CryptoCell disabled, seed generated");
        seed
    };

    if !settings_ref.general.name.is_empty() {
        ble::peripheral::set_device_name_initial(settings_ref.general.name.as_str());
    }
    let controller = ble::host::init(embassy_nrf::ipc::Ipc::new(p.IPC, Irqs)).await;
    spawner.spawn(ble_task(controller, ble_seed).unwrap());

    defmt::info!("Spawning power_state_task...");
    spawner.spawn(
        power_state::power_state_task(
            pmic,
            pmic_leds,
            &atslite_board::battery_model::BATTERY_MODEL,
        )
        .unwrap(),
    );

    let ctx = ObjectModeContext::<AtslitePlatform, _, _> {
        paj7025r2_group: (paj_r2_mutex, board.paj7025r2.fod.into(), p.GPIOTE_CH1),
        paj7025r3_group: (paj_r3_mutex, board.paj7025r3.fod.into(), p.GPIOTE_CH2),
        fod_set_ch: p.PPI_CH1.into(),
        fod_clr_ch: p.PPI_CH2.into(),
        timer: p.TIMER0,
        imu,
        imu_int,
        usb_snd,
        usb_rcv,
        usb_configured: usb_cfg,
        settings: settings_ref,
        l2cap_channels,
        data_mode: active_transport,
        version: FIRMWARE_VERSION,
        battery_soc: &atslite_board::power_state::SOC_PERCENT,
        battery_charging: &atslite_board::power_state::VBUS_PRESENT,
    };

    defmt::info!("Initialization complete; entering object mode");
    if let Err(e) = object_mode(0x5210, ctx).await {
        defmt::error!(
            "object_mode exited with error: {:?}",
            defmt::Debug2Format(&e)
        );
    }
    loop {
        Timer::after_secs(1).await;
    }
}

#[embassy_executor::task]
async fn pmic_irq_task(
    irq_pin: embassy_nrf::Peri<'static, embassy_nrf::gpio::AnyPin>,
    pmic: &'static AsyncMutex<NoopRawMutex, NPM1300<Twim<'static>, Delay>>,
) {
    let port = irq_pin.port();
    let pin = irq_pin.pin();
    utils::set_pin_sense(&port, pin, pac::gpio::vals::Sense::HIGH);
    let mut irq = gpio::Input::new(irq_pin, gpio::Pull::None);

    loop {
        irq.wait_for_high().await;
        if !pmic
            .lock()
            .await
            .get_vbus_in_status()
            .await
            .unwrap()
            .is_vbus_in_present
        {
            pmic.lock()
                .await
                .clear_vbusin0_event_mask(npm1300_rs::mainreg::Vbusin0EventMask::VBUS_REMOVED)
                .await
                .unwrap();
            defmt::debug!("VBUS removed");
            power::notify_vbus_removed();
        }
        pmic.lock()
            .await
            .clear_vbusin1_event_mask(
                npm1300_rs::mainreg::Vbusin1EventMask::CC1_STATE_CHANGE
                    | npm1300_rs::mainreg::Vbusin1EventMask::CC2_STATE_CHANGE,
            )
            .await
            .unwrap();
        let cc_status = pmic.lock().await.get_vbus_cc_status().await.unwrap();
        use npm1300_rs::sysreg::VbusInCcCmp;
        let high_power = matches!(
            cc_status.vbusin_cc1_status,
            VbusInCcCmp::MA1500HighPower | VbusInCcCmp::MA3000HighPower
        ) || matches!(
            cc_status.vbusin_cc2_status,
            VbusInCcCmp::MA1500HighPower | VbusInCcCmp::MA3000HighPower
        );
        if high_power {
            defmt::debug!("High-power CC detected -> raising limit");
            pmic.lock()
                .await
                .set_vbus_in_current_limit(npm1300_rs::sysreg::VbusInCurrentLimit::MA500)
                .await
                .unwrap();
        }
    }
}

#[embassy_executor::task]
async fn led_task(mut anim: PmicLedAnimator<'static, Twim<'static>, Delay>) -> ! {
    anim.run().await
}

#[embassy_executor::task]
async fn watchdog_feeder(mut wdt_handle: embassy_nrf::wdt::WatchdogHandle) {
    loop {
        embassy_time::Timer::after_secs(30).await;
        wdt_handle.pet();
    }
}

#[embassy_executor::task]
async fn power_button_task(
    btn: Input<'static>,
    pmic: &'static AsyncMutex<NoopRawMutex, NPM1300<Twim<'static>, Delay>>,
    leds: &'static PmicLedsHandle,
) {
    power_button::power_button_loop(btn, pmic, leds).await;
}

#[embassy_executor::task]
async fn ble_task(controller: ble::host::BleController, seed: [u8; 32]) {
    ble::peripheral::run(controller, seed, "ATS USB", b"ATS").await;
}

#[embassy_executor::task]
async fn usb_pmic_config_task(
    configured: common::usb::UsbConfigHandle,
    pmic: &'static AsyncMutex<NoopRawMutex, NPM1300<Twim<'static>, Delay>>,
) {
    configured.wait_until_active().await;
    pmic.lock()
        .await
        .set_vbus_in_current_limit(npm1300_rs::sysreg::VbusInCurrentLimit::MA500)
        .await
        .unwrap();
    defmt::info!("USB configured, PMIC current limit set to 500mA");
}

fn active_transport() -> TransportMode {
    atslite_transport::report_transport(usb::is_active())
}

async fn handle_pending_ship(
    pmic: &'static AsyncMutex<NoopRawMutex, NPM1300<Twim<'static>, Delay>>,
    leds: &PmicLedsHandle,
    pwr_btn: &mut Input<'_>,
) -> Result<
    (),
    npm1300_rs::NPM1300Error<<Twim<'static> as embedded_hal_async::i2c::ErrorType>::Error>,
> {
    if !power::take_pending_ship_flag() {
        return Ok(());
    }
    let vbus_present = pmic
        .lock()
        .await
        .get_vbus_in_status()
        .await?
        .is_vbus_in_present;
    if !vbus_present {
        defmt::trace!("Pending ship with no VBUS -> ship now");
        leds.set_state(LedState::Off).await;
        pmic.lock().await.enter_ship_mode().await?;
        loop {
            cortex_m::asm::wfe();
        }
    }
    defmt::trace!("Pending ship but VBUS present -> wait for removal");
    // Faux-off with VBUS present keeps the red charging LED on.
    leds.set_state(LedState::BattCharging).await;
    embassy_futures::select::select(
        async {
            power::VBUS_REMOVED_SIG.wait().await;
            pmic.lock().await.enter_ship_mode().await.unwrap();
            loop {}
        },
        power_button::wait_for_power_button_full_press(
            pwr_btn,
            embassy_time::Duration::from_millis(96),
        ),
    )
    .await;
    Ok(())
}

async fn handle_boot_vbus_policy(
    pmic: &'static AsyncMutex<NoopRawMutex, NPM1300<Twim<'static>, Delay>>,
    leds: &PmicLedsHandle,
) -> Result<
    (),
    npm1300_rs::NPM1300Error<<Twim<'static> as embedded_hal_async::i2c::ErrorType>::Error>,
> {
    use embassy_nrf::reset::ResetReason;
    let rr = embassy_nrf::reset::read_reasons();
    embassy_nrf::reset::clear_reasons();
    defmt::trace!("Reset reasons: {:?}", defmt::Debug2Format(&rr));

    let vbus_present = pmic
        .lock()
        .await
        .get_vbus_in_status()
        .await?
        .is_vbus_in_present;
    let woke_by_gpio = rr.contains(ResetReason::RESETPIN);
    let woke_by_vbus = rr.contains(ResetReason::VBUS);

    if woke_by_gpio {
        defmt::trace!("Booted via GPIO");
        return Ok(());
    }

    if woke_by_vbus || (rr.is_empty() && vbus_present) {
        if vbus_present {
            leds.set_state(LedState::BattCharging).await;
        } else {
            leds.set_state(LedState::Off).await;
        }
        power::set_pending_ship_flag();
    }
    Ok(())
}
