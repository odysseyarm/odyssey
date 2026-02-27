//! Lite1 board main firmware
//!
//! ATSlite variant with PAG7665QN vision sensor.
//! USB vendor endpoints and BLE with transport mode switching.

#![no_std]
#![no_main]
#![feature(never_type)]

use defmt_rtt as _;
use panic_probe as _;

use lite1::transport_mode;

use better_dfu::consts::DfuAttributes;
use better_dfu::{Control, ResetImmediate, application::DfuMarker, usb_dfu};
use core::cell::RefCell;
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_executor::Spawner;
use embassy_nrf::{
    gpio::{self, Input, Level, Output, OutputDrive, Pin, Pull},
    nvmc::Nvmc,
    pac, peripherals,
    spim::{self, Spim},
    twim::{self, Twim},
};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::{Delay, Timer};
use embassy_usb::msos;
use embedded_hal_bus::spi::ExclusiveDevice;
use npm1300_rs::NPM1300;
use pag7665qn::{Pag7665Qn, mode};
use static_cell::StaticCell;

use donger_common::usb_device as usb;
use lite1::{
    Irqs, ble, imu, object_mode,
    pins::Board,
    pmic_leds::{LedIdx, LedState, PmicLedAnimator, PmicLedsHandle},
    power, power_button, power_state, sensors, split_board, utils,
};

const DEVICE_INTERFACE_GUID: &str = "{A4769731-EC56-49FF-9924-613E5B3D4D6C}";
const DFU_INTERFACE_GUID: &str = "{72DC6483-1013-4BC3-B1CF-6A02DDAEFCE5}";

const FIRMWARE_VERSION: [u16; 3] = [
    env!("CARGO_PKG_VERSION_MAJOR").as_bytes()[0] as u16 - b'0' as u16,
    env!("CARGO_PKG_VERSION_MINOR").as_bytes()[0] as u16 - b'0' as u16,
    env!("CARGO_PKG_VERSION_PATCH").as_bytes()[0] as u16 - b'0' as u16,
];

// Default accelerometer ODR in Hz (matches legacy atslite)
const DEFAULT_ACCEL_ODR: u16 = 100;

struct AppDfuMarker<'a> {
    flash: &'a Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>,
}

impl<'a> DfuMarker for AppDfuMarker<'a> {
    fn mark_dfu(&mut self) {
        defmt::info!("DFU Detach");
        let result = self.flash.lock(|flash_cell| {
            let mut flash = flash_cell.borrow_mut();
            lite1_boot_api::BootConfirmation::request_dfu_with(&mut *flash)
        });
        match result {
            Ok(()) => defmt::info!("DFU requested - SUCCESS"),
            Err(()) => defmt::warn!("DFU requested - FAILED"),
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    defmt::info!("Lite1 board initialization starting...");

    defmt::info!("Calling nrf5340_init::init()...");
    let (_cp, p) = lite1::nrf5340_init::init();
    defmt::info!("nrf5340_init complete");

    defmt::info!("Splitting board peripherals...");
    let board: Board = split_board!(p);
    defmt::info!("Board peripherals split");

    // Initialize PMIC over TWIM
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

    // Handle PMIC IRQ pin
    defmt::info!("Spawning pmic_irq_task...");
    spawner.spawn(pmic_irq_task(board.pmic.int.into(), pmic).unwrap());

    // Initialize PMIC LED animator
    defmt::info!("Creating PmicLedAnimator...");
    let (anim, handle) = PmicLedAnimator::new(pmic, LedIdx::Ch0, LedIdx::Ch1, LedIdx::Ch2)
        .await
        .unwrap();
    defmt::info!("Spawning led_task...");
    spawner.spawn(led_task(anim).unwrap());
    static PMIC_LEDS_HANDLE: StaticCell<PmicLedsHandle> = StaticCell::new();
    let pmic_leds = PMIC_LEDS_HANDLE.init(handle);

    // Power button handling
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
    defmt::info!("Spawning power_state_task...");
    spawner.spawn(
        power_state::power_state_task(pmic, pmic_leds, &lite1::battery_model::BATTERY_MODEL)
            .unwrap(),
    );
    defmt::info!("Power tasks spawned");

    // Initialize NVMC in a shared mutex so it can be used by both DFU and settings
    defmt::info!("Initializing NVMC...");
    static NVMC: StaticCell<Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>> = StaticCell::new();
    let nvmc = NVMC.init_with(|| Mutex::new(RefCell::new(Nvmc::new(p.NVMC))));
    defmt::info!("NVMC initialized");

    // Initialize settings storage (must be early so PAG settings can be restored)
    defmt::info!("Initializing settings...");
    let nvmc_async =
        nvmc.lock(|_| BlockingAsync::new(unsafe { Nvmc::new(peripherals::NVMC::steal()) }));
    lite1::settings::init_core(&spawner, nvmc_async).await;
    let settings = lite1::settings::init_nodes().await;
    lite1::settings::init_board_nodes(settings).await;
    defmt::info!("Settings initialized");

    // Reset the PAG7665QN
    defmt::info!("Resetting PAG7665QN...");
    let mut vis_resetn = Output::new(board.pag7665qn.reset, Level::Low, OutputDrive::Standard);
    Timer::after_micros(500).await;
    vis_resetn.set_high();
    Timer::after_millis(30).await;

    // Initialize PAG7665QN via SPIM4 (high-speed dedicated SPI)
    defmt::info!("Initializing PAG7665QN...");
    // Erratum 135 workaround
    unsafe {
        (0x5000ac04 as *mut u32).write_volatile(1);
    }
    let mut spim_config = spim::Config::default();
    spim_config.frequency = spim::Frequency::M16;
    spim_config.mode = spim::MODE_3;
    spim_config.bit_order = spim::BitOrder::MSB_FIRST;

    let spim = Spim::new(
        p.SPIM4,
        Irqs,
        board.pag7665qn.sck,
        board.pag7665qn.miso,
        board.pag7665qn.mosi,
        spim_config,
    );
    let cs = Output::new(board.pag7665qn.cs, Level::High, OutputDrive::Standard);
    let int_o = Input::new(board.pag7665qn.int_o, Pull::None);
    let spi_device = ExclusiveDevice::new(spim, cs, Delay).unwrap();

    let (mut pag, pag_int_wrapper) = Pag7665Qn::init_spi(spi_device, Delay, int_o, mode::Idle)
        .await
        .unwrap();
    defmt::info!("PAG7665QN initialized");

    // Apply saved PAG settings (or defaults if first boot)
    defmt::info!("Applying PAG7665QN settings from flash...");
    let pag_settings = &settings.pag;
    pag.set_sensor_fps(pag_settings.frame_rate).await.unwrap();
    pag.set_sensor_gain(pag_settings.gain).await.unwrap();
    // Exposure register: bit[7]=LED_Always_ON, bits[6:0]=time (units of 100µs)
    let led_always_on = pag_settings.exposure & 0x80 != 0;
    let exposure_us = u16::from(pag_settings.exposure & 0x7F) * 100;
    pag.set_sensor_exposure_us(led_always_on, exposure_us)
        .await
        .unwrap();
    pag.set_area_lower_bound(pag_settings.area_min)
        .await
        .unwrap();
    pag.set_area_upper_bound(pag_settings.area_max)
        .await
        .unwrap();
    pag.set_light_threshold(pag_settings.light_threshold)
        .await
        .unwrap();
    pag.set_circle_params(pag7665qn::types::CircleParams {
        r_lower_bound: pag_settings.circle_r_min,
        r_upper_bound: pag_settings.circle_r_max,
        k_lower_bound: pag_settings.circle_k_min,
        k_upper_bound: pag_settings.circle_k_max,
    })
    .await
    .unwrap();
    defmt::info!(
        "PAG7665QN settings applied: fps={}, gain={}, exposure={}us, area={}-{}, light_thrd={}",
        pag_settings.frame_rate,
        pag_settings.gain,
        exposure_us,
        pag_settings.area_min,
        pag_settings.area_max,
        pag_settings.light_threshold
    );

    // Wrap PAG in sensor wrapper for object_mode
    let pag_sensor = sensors::Pag7665QnSensor::new(pag.as_dynamic_mode());
    static PAG_MUTEX: StaticCell<AsyncMutex<NoopRawMutex, sensors::Pag7665QnSensor>> =
        StaticCell::new();
    let pag_mutex = PAG_MUTEX.init(AsyncMutex::new(pag_sensor));
    let pag_int = sensors::Pag7665QnInterrupt {
        int_o: pag_int_wrapper.int_o,
    };

    // Initialize IMU (icm42688)
    defmt::info!("Initializing IMU...");
    let (imu_driver, imu_int_input) = imu::init(
        p.SERIAL0,
        board.icm42688p.cs.into(),
        board.icm42688p.sck.into(),
        board.icm42688p.miso.into(),
        board.icm42688p.mosi.into(),
        board.icm42688p.int1.into(),
        board.icm42688p.clkin.into(),
        p.TIMER1,
        p.PPI_CH0.into(),
        p.GPIOTE_CH0,
        DEFAULT_ACCEL_ODR,
    )
    .await;
    defmt::info!("IMU initialized");

    // Wrap IMU in sensor wrapper for object_mode
    let imu_sensor = sensors::Icm42688Sensor::new(imu_driver);
    let imu_int = sensors::Icm42688Interrupt {
        int1: imu_int_input.int1,
    };

    // Confirm boot using boot-api (caches state for DFU requests)
    nvmc.lock(|flash_cell| {
        let mut flash = flash_cell.borrow_mut();
        lite1_boot_api::BootConfirmation::mark_booted(&mut *flash).expect("Failed to mark boot");
    });
    defmt::info!("Boot confirmed");

    // USB initialization with DFU runtime interface
    static DFU_STATE: StaticCell<Control<AppDfuMarker<'static>, ResetImmediate>> =
        StaticCell::new();
    let dfu_state = DFU_STATE.init(Control::new(
        AppDfuMarker { flash: nvmc },
        DfuAttributes::CAN_DOWNLOAD | DfuAttributes::WILL_DETACH,
        ResetImmediate,
    ));

    defmt::info!("Initializing USB...");
    let vbus = embassy_nrf::usb::vbus_detect::HardwareVbusDetect::new(Irqs);
    let (usb_dev, usb_snd, usb_rcv, usb_cfg) = usb::usb_device(
        0x5211, // Lite1 PID
        "ATS USB",
        DEVICE_INTERFACE_GUID,
        p.USBD,
        Irqs,
        vbus,
        lite1::device_control::try_send_cmd,
        lite1::device_control::try_recv_event,
        |builder| {
            usb_dfu(
                builder,
                dfu_state,
                embassy_time::Duration::from_millis(2500),
                |func| {
                    func.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
                    func.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
                        "DeviceInterfaceGUIDs",
                        msos::PropertyData::RegMultiSz(&[DFU_INTERFACE_GUID]),
                    ));
                },
            );
        },
    );
    spawner.spawn(usb::run_usb(usb_dev).unwrap());

    // Initialize device control channels and spawn the task
    let (ctrl_evt_ch, ctrl_cmd_ch) = lite1::device_control::init();
    lite1::device_control::register(ctrl_evt_ch, ctrl_cmd_ch);

    // Register the board-specific persist function for transport mode before spawning the task
    lite1::device_control_task::register_transport_mode_persist_fn(
        lite1::settings::persist_transport_mode,
    );
    spawner.spawn(lite1::device_control_task::device_control_task(FIRMWARE_VERSION).unwrap());

    // Load transport mode from persisted settings
    let initial_transport_mode = settings.general.transport_mode;
    transport_mode::set(initial_transport_mode);
    defmt::info!("Transport mode initialized from settings: {:?}", initial_transport_mode);

    // Adjust PMIC limit after USB enumerates
    spawner.spawn(usb_pmic_config_task(usb_cfg, pmic).unwrap());

    // Generate BLE seed from CryptoCell
    defmt::info!("Generating BLE seed from CryptoCell...");
    let ble_seed = {
        use embassy_nrf::cryptocell_rng::CcRng;
        let mut seed = [0u8; 32];
        let mut cc_rng = CcRng::new_blocking(p.CC_RNG);
        cc_rng.blocking_fill_bytes(&mut seed);
        drop(cc_rng);
        embassy_nrf::pac::CRYPTOCELL
            .enable()
            .write(|w| w.set_enable(false));
        seed
    };

    // BLE controller and tasks (settings must be initialized first for bond restoration)
    let controller = ble::host::init(embassy_nrf::ipc::Ipc::new(p.IPC, Irqs)).await;
    spawner.spawn(ble_task(controller, ble_seed).unwrap());

    // Initialize watchdog (started by stage2 bootloader, we just need to pet it)
    defmt::info!("Initializing watchdog...");
    use embassy_nrf::wdt::{Config as WdtConfig, HaltConfig, SleepConfig, Watchdog};
    let mut wdt_config = WdtConfig::default();
    wdt_config.timeout_ticks = 32768 * 60; // 60 second timeout (must match bootloader)
    wdt_config.action_during_sleep = SleepConfig::RUN;
    wdt_config.action_during_debug_halt = HaltConfig::PAUSE;
    let (_wdt, [wdt_handle]) = Watchdog::try_new(p.WDT0, wdt_config).expect("Watchdog init failed");
    spawner.spawn(watchdog_feeder(wdt_handle).unwrap());

    pmic_leds.set_state(LedState::BattHigh).await;
    defmt::info!("Initialization complete, starting object mode");

    // Get device ID for object mode
    let device_id: [u8; 6] = utils::device_id()[..6].try_into().unwrap();

    // Run object mode
    let ctx = object_mode::ObjectModeContext {
        pag: pag_mutex,
        pag_int,
        imu: imu_sensor,
        imu_int,
        device_id,
        product_id: 0x5211,
        firmware_version: FIRMWARE_VERSION,
        settings,
        usb_snd,
        usb_rcv,
        usb_configured: usb_cfg,
    };

    object_mode::run(ctx).await;
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
async fn power_button_task(
    btn: Input<'static>,
    pmic: &'static AsyncMutex<NoopRawMutex, NPM1300<Twim<'static>, Delay>>,
    leds: &'static PmicLedsHandle,
) {
    power_button::power_button_loop(btn, pmic, leds).await;
}

#[embassy_executor::task]
async fn ble_task(controller: ble::host::BleController, seed: [u8; 32]) {
    ble::peripheral::run(controller, seed).await;
}

#[embassy_executor::task]
async fn usb_pmic_config_task(
    configured: donger_common::usb_device::UsbConfigHandle,
    pmic: &'static AsyncMutex<NoopRawMutex, NPM1300<Twim<'static>, Delay>>,
) {
    configured.wait_until_configured().await;
    pmic.lock()
        .await
        .set_vbus_in_current_limit(npm1300_rs::sysreg::VbusInCurrentLimit::MA500)
        .await
        .unwrap();
    defmt::info!("USB configured, PMIC current limit set to 500mA");
}

#[embassy_executor::task]
async fn watchdog_feeder(mut wdt_handle: embassy_nrf::wdt::WatchdogHandle) {
    loop {
        Timer::after_secs(30).await;
        wdt_handle.pet();
    }
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
