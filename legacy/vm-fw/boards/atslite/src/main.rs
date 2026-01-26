#![no_main]
#![no_std]
#![feature(never_type)]

use defmt_rtt as _;
use panic_probe as _;

use atslite_board::transport_mode;

use better_dfu::consts::DfuAttributes;
use better_dfu::{Control, ResetImmediate, application::DfuMarker, usb_dfu};
use core::cell::RefCell;
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_executor::Spawner;
use embassy_nrf::{
    gpio::{self, Input, Pin},
    nvmc::Nvmc,
    pac, peripherals,
    twim::{self, Twim},
};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::{Delay, Timer};
use npm1300_rs::{self, NPM1300};
use protodongers::control::device::TransportMode;
use static_cell::StaticCell;

use atslite_board::{
    Irqs, ble, embassy_executor, embassy_nrf, imu,
    l2cap::AtsliteL2capChannels,
    nrf5340_init,
    pins::Board,
    platform::AtslitePlatform,
    pmic_leds::{LedIdx, LedState, PmicLedAnimator, PmicLedsHandle},
    power, power_button, power_state, split_board, utils,
};
use common::{
    Paj,
    object_mode::{ObjectModeContext, object_mode},
    settings, usb,
};
use embassy_usb::msos;

const DEVICE_INTERFACE_GUID: &str = "{A4769731-EC56-49FF-9924-613E5B3D4D6C}";

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

// Marker used by DFU runtime detach to request bootloader DFU on next reset.
struct AppDfuMarker<'a> {
    flash: &'a Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>,
}

impl<'a> DfuMarker for AppDfuMarker<'a> {
    fn mark_dfu(&mut self) {
        defmt::info!("Dfu Detach");
        // Log boot state before requesting DFU
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

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    defmt::info!("ATSlite board initialization starting...");

    let (_cp, p) = nrf5340_init::init();

    // Initialize NVMC in a StaticCell so it can be shared between DFU and settings
    static NVMC: StaticCell<Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>> = StaticCell::new();
    let nvmc = NVMC.init_with(|| Mutex::new(RefCell::new(Nvmc::new(p.NVMC))));

    let board: Board = split_board!(p);

    // Boot confirmation moved to later (after IMU init)

    // Initialize PMIC over TWIM
    let mut twim_cfg = twim::Config::default();
    twim_cfg.frequency = twim::Frequency::K400;
    static TWIM_BUF: StaticCell<[u8; 0]> = StaticCell::new();
    let twim_buf = TWIM_BUF.init([0u8; 0]);
    let twim = Twim::new(
        p.SERIAL2,
        Irqs,
        board.pmic.sda,
        board.pmic.scl,
        twim_cfg,
        twim_buf,
    );

    static PMIC: StaticCell<AsyncMutex<NoopRawMutex, NPM1300<Twim<'static>, Delay>>> =
        StaticCell::new();
    let mut pmic_dev = NPM1300::new(twim, Delay);
    power::pmic_setup(&mut pmic_dev).await.unwrap();
    power::configure_and_start_charging(
        &mut pmic_dev,
        npm1300_rs::sysreg::VbusInCurrentLimit::MA100,
    )
    .await
    .unwrap();
    let pmic = PMIC.init(AsyncMutex::new(pmic_dev));

    // Handle PMIC IRQ pin
    spawner
        .spawn(pmic_irq_task(board.pmic.irq.into(), pmic))
        .unwrap();

    // Initialize PMIC LED animator
    let (anim, handle) = PmicLedAnimator::new(pmic, LedIdx::Ch0, LedIdx::Ch1, LedIdx::Ch2)
        .await
        .unwrap();
    spawner.spawn(led_task(anim)).unwrap();
    static PMIC_LEDS_HANDLE: StaticCell<PmicLedsHandle> = StaticCell::new();
    let pmic_leds = PMIC_LEDS_HANDLE.init(handle);

    // Power button handling
    let mut pwr_btn = Input::new(board.pmic.pwr_btn, gpio::Pull::Up);
    handle_boot_vbus_policy(pmic, pmic_leds).await.unwrap();
    handle_pending_ship(pmic, pmic_leds, &mut pwr_btn)
        .await
        .unwrap();
    pmic_leds.set_state(LedState::TurningOn).await;
    let pwr_btn_task = pwr_btn;
    spawner
        .spawn(power_button_task(pwr_btn_task, pmic, pmic_leds))
        .unwrap();
    spawner
        .spawn(power_state::power_state_task(pmic, pmic_leds))
        .unwrap();

    // Initialize PAJ sensors
    let mut paj_cfg = embassy_nrf::spim::Config::default();
    paj_cfg.frequency = embassy_nrf::spim::Frequency::M8;
    paj_cfg.mode = embassy_nrf::spim::MODE_3;
    paj_cfg.bit_order = embassy_nrf::spim::BitOrder::LSB_FIRST;

    // Manually construct SPI pins, leaving fod intact
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

    let dev_r2 = common::utils::make_spi_dev(p.SERIAL0, Irqs, paj7025r2_spi, paj_cfg.clone());
    let dev_r3 = common::utils::make_spi_dev(p.SERIAL1, Irqs, paj7025r3_spi, paj_cfg);

    Timer::after_millis(1).await;

    let mut paj7025r2 = Paj::init(dev_r2).await.unwrap();
    let mut paj7025r3 = Paj::init(dev_r3).await.unwrap();
    paj7025r2.init_settings(true, 0x10, 0x00).await.unwrap();
    paj7025r3.init_settings(true, 0x10, 0x02).await.unwrap();

    static PAJ7025R2_MUTEX: StaticCell<AsyncMutex<NoopRawMutex, Paj>> = StaticCell::new();
    static PAJ7025R3_MUTEX: StaticCell<AsyncMutex<NoopRawMutex, Paj>> = StaticCell::new();
    let paj_r2_mutex = PAJ7025R2_MUTEX.init(AsyncMutex::new(paj7025r2));
    let paj_r3_mutex = PAJ7025R3_MUTEX.init(AsyncMutex::new(paj7025r3));

    // Initialize IMU (icm42688)
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
        p.TIMER1, // Use TIMER1 for IMU, save TIMER0 for object mode
        p.PPI_CH0.into(),
        p.GPIOTE_CH0,
    )
    .await;

    // Confirm boot using boot-api wrapper (which caches state for DFU requests)
    log_boot_state("before mark_booted");

    nvmc.lock(|flash_cell| {
        let mut flash = flash_cell.borrow_mut();
        atslite_boot_api::BootConfirmation::mark_booted(&mut *flash).expect("Failed to mark boot");
    });

    log_boot_state("after mark_booted");

    // USB initialization with DFU runtime interface
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
        DEVICE_INTERFACE_GUID,
        p.USBD,
        Irqs,
        vbus,
        |builder| {
            // Add DFU runtime interface for detach support
            usb_dfu(
                builder,
                dfu_state,
                embassy_time::Duration::from_millis(2500),
                |func| {
                    // Apply WINUSB to DFU interface for Windows compatibility
                    func.msos_feature(embassy_usb::msos::CompatibleIdFeatureDescriptor::new(
                        "WINUSB", "",
                    ));
                    func.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
                        "DeviceInterfaceGUIDs",
                        msos::PropertyData::RegMultiSz(&[DEVICE_INTERFACE_GUID]),
                    ));
                },
            );
        },
    );
    spawner.spawn(usb::run_usb(usb_dev)).unwrap();

    // CRITICAL: Wait for flash writes to fully settle before stealing NVMC peripheral.
    // On nRF5340, flash writes continue in hardware after NVMC API returns.
    // Reinitializing NVMC (via new()) while flash controller is busy corrupts recent writes.
    // This 200ms delay ensures boot confirmation write at 0xD000 is fully complete.
    Timer::after_millis(200).await;

    // Settings flash - borrow NVMC from the shared mutex
    let nvmc_async = nvmc.lock(|_f| {
        // Create a new Nvmc by stealing the peripheral - this is safe because
        // we're the only user during normal operation. On DFU DETACH, the device
        // will reset, so there's no conflict.
        BlockingAsync::new(unsafe { Nvmc::new(peripherals::NVMC::steal()) })
    });
    let settings_ref = {
        let mut r2 = paj_r2_mutex.lock().await;
        let mut r3 = paj_r3_mutex.lock().await;
        settings::init_settings(&spawner, nvmc_async, &mut *r2, &mut *r3)
            .await
            .unwrap()
    };

    // Check boot state after settings init to see if corruption happened
    log_boot_state("after settings init");

    // Default transport mode to BLE after settings are initialized
    transport_mode::set(TransportMode::Ble);
    defmt::info!("Transport mode initialized to BLE");

    // Initialize device control channels after settings are ready
    let (ctrl_evt_ch, ctrl_cmd_ch) = common::device_control::init();
    common::device_control::register(ctrl_evt_ch, ctrl_cmd_ch);
    spawner
        .spawn(atslite_board::device_control_task::device_control_task())
        .unwrap();

    // Confirm this boot as successful so the bootloader keeps the current slot active.
    // Adjust PMIC limit after USB enumerates
    spawner.spawn(usb_pmic_config_task(usb_cfg, pmic)).unwrap();

    // BLE controller and tasks
    let controller = ble::host::init(embassy_nrf::ipc::Ipc::new(p.IPC, Irqs)).await;
    spawner.spawn(ble_task(controller)).unwrap();

    // Initialize watchdog (started by stage2 bootloader)
    use embassy_nrf::wdt::{Config as WdtConfig, HaltConfig, SleepConfig, Watchdog};
    let mut wdt_config = WdtConfig::default();
    wdt_config.timeout_ticks = 32768 * 60;
    wdt_config.action_during_sleep = SleepConfig::RUN;
    wdt_config.action_during_debug_halt = HaltConfig::PAUSE;
    let (_wdt, [wdt_handle]) = Watchdog::try_new(p.WDT0, wdt_config).expect("Watchdog init failed");
    spawner.spawn(watchdog_feeder(wdt_handle)).unwrap();
    let l2cap_channels = AtsliteL2capChannels::new();

    // Build object mode context
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
        data_mode: transport_mode::get,
        version: utils::FIRMWARE_VERSION,
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
async fn ble_task(controller: ble::host::BleController) {
    ble::peripheral::run(controller).await;
}

#[embassy_executor::task]
async fn usb_pmic_config_task(
    configured: common::usb::UsbConfigHandle,
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
    // Keep red charging LED on in faux-off mode with VBUS present
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
