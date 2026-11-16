#![no_main]
#![no_std]
#![expect(
    unsafe_code,
    reason = "Firmware entrypoint coordinates raw channel pointers, logger state, and interrupt handoff"
)]

mod ble;
mod control;
#[cfg(not(feature = "rtt"))]
mod defmt_serial;
mod pairing;
mod storage;
mod usb;

use core::sync::atomic::{AtomicBool, Ordering};

use cortex_m_rt::{ExceptionFrame, exception};
use defmt::{error, info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_nrf::peripherals::RNG as NRF_RNG;
use embassy_nrf::{bind_interrupts, peripherals, rng, usb as nrf_usb};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal as SyncSignal;
#[cfg(feature = "rtt")]
use {defmt_rtt as _, panic_probe as _};

// Control-plane replies to the host (RequestDevices, ReadVersion, ...).
type HostResponseChannel = Channel<ThreadModeRawMutex, MuxMsg, 8>;
pub static HOST_RESPONSES: HostResponseChannel = Channel::new();

// Set while the host wants a DevicesSnapshot on every connect and disconnect.
pub static DEVICE_LIST_SUBSCRIBED: AtomicBool = AtomicBool::new(false);
use embassy_usb::driver::{EndpointError, EndpointOut};
use nrf_sdc::{self as sdc, mpsl};
use odyssey_protocol::control::usb_mux::{
    BondStoreError, ClearBondsResult, PairingError, PairingOutcome, UsbMuxCtrlMsg,
};
use odyssey_protocol::mux::{DevicePacket, MuxMsg, Version};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use static_cell::StaticCell;
use trouble_host::PacketPool;
use trouble_host::prelude::DefaultPacketPool;

type Controller = nrf_sdc::SoftdeviceController<'static>;
type Pool = DefaultPacketPool;

use heapless::Vec as HVec;

use crate::ble::{ACTIVE_CONNECTIONS, ActiveConnections, BleManager, DevicePacketChannel};
use crate::storage::{BondData, Settings};

// Commands for the task that owns the host's bond database, which is preloaded
// at boot. ClearBonds sends PurgeBonds here and waits on HOST_MGMT_DONE before
// replying, so the host never keeps an LTK the dongle has forgotten.
enum HostMgmtCmd {
    PurgeBonds,
    AddBond(BondData),
}

static HOST_MGMT_CH: Channel<ThreadModeRawMutex, HostMgmtCmd, 2> = Channel::new();
static HOST_MGMT_DONE: SyncSignal<ThreadModeRawMutex, ()> = SyncSignal::new();
static HOST_MGMT_ADD_RESULT: SyncSignal<ThreadModeRawMutex, Result<(), ()>> = SyncSignal::new();
static BONDS_CLEARED: SyncSignal<ThreadModeRawMutex, ()> = SyncSignal::new();
// Feeds every advertisement into the name cache and, while pairing, reports
// the first one that matches the pairing filter.
struct HostEventHandler;
impl trouble_host::prelude::EventHandler for HostEventHandler {
    fn on_adv_reports(&self, reports: trouble_host::scan::LeAdvReportsIter) {
        for r in reports {
            if let Ok(rep) = r {
                let raw = rep.addr.raw();
                let mut addr = [0u8; 6];
                addr.copy_from_slice(raw);
                let name = crate::ble::central::extract_local_name(rep.data);
                crate::ble::accessories::advertisement(
                    trouble_host::Address {
                        kind: rep.addr_kind,
                        addr: rep.addr,
                    },
                    rep.data,
                );
                // Cache names even outside pairing. ListBondsResponse reads them.
                if !name.is_empty() {
                    crate::ble::central::update_device_name(addr, name.clone());
                }
                if crate::pairing::is_active() && crate::ble::central::adv_matches(rep.data) {
                    defmt::info!("pairing: adv/scan match {:02x}", addr);
                    crate::ble::central::report_scan_address(addr, name);
                    break;
                }
            }
        }
    }
    fn on_ext_adv_reports(&self, reports: trouble_host::scan::LeExtAdvReportsIter) {
        for r in reports {
            if let Ok(rep) = r {
                let raw = rep.addr.raw();
                let mut addr = [0u8; 6];
                addr.copy_from_slice(raw);
                let name = crate::ble::central::extract_local_name(rep.data);
                crate::ble::accessories::advertisement(
                    trouble_host::Address {
                        kind: rep.addr_kind,
                        addr: rep.addr,
                    },
                    rep.data,
                );
                if !name.is_empty() {
                    crate::ble::central::update_device_name(addr, name.clone());
                }
                if crate::pairing::is_active() && crate::ble::central::adv_matches(rep.data) {
                    defmt::info!("pairing: adv match (EXT) {:02x}", addr);
                    crate::ble::central::report_scan_address(addr, name);
                    break;
                }
            }
        }
    }
}
static HOST_HANDLER: HostEventHandler = HostEventHandler;

bind_interrupts!(pub struct Irqs {
    // CLOCK_POWER is shared by USB VBUS detect and MPSL.
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler, mpsl::ClockInterruptHandler;

    EGU0_SWI0 => mpsl::LowPrioInterruptHandler;
    RADIO => mpsl::HighPrioInterruptHandler;
    TIMER0 => mpsl::HighPrioInterruptHandler;
    RTC0 => mpsl::HighPrioInterruptHandler;

    RNG => rng::InterruptHandler<NRF_RNG>;
    USBD => nrf_usb::InterruptHandler<peripherals::USBD>;
});

const FIRMWARE_SEMVER: [u32; 3] = odyssey_version::cargo_version!();

#[exception]
unsafe fn HardFault(_ef: &ExceptionFrame) -> ! {
    // A panic ends up here too. Resets instead of hanging.
    cortex_m::peripheral::SCB::sys_reset();
}

#[embassy_executor::main]
async fn main(mut spawner: Spawner) {
    defmt::info!(
        "Booted {=str} build {=str}",
        env!("CARGO_PKG_VERSION"),
        env!("GIT_DESCRIBE")
    );
    let p = embassy_nrf::init(Default::default());

    // "Red" status LED on P0.08, "blue" pairing LED on P0.14. The nRF52840-DK's
    // LEDs (all white) are on P0.13-P0.16.
    let red = p.P0_08;

    let mut red = embassy_nrf::gpio::Output::new(
        red,
        embassy_nrf::gpio::Level::High,
        embassy_nrf::gpio::OutputDrive::Standard,
    );

    red.set_low();

    // Default interrupt priorities are fine.

    #[cfg(feature = "rtt")]
    let (ep_in, ep_out) = {
        use static_cell::StaticCell;

        // With RTT the USB device carries no serial logger.
        let (builder, ep_in, ep_out) = usb::usb_device(p.USBD);
        let usb_device = builder.build();

        static USB_DEVICE: StaticCell<embassy_usb::UsbDevice<'static, usb::UsbDriver>> =
            StaticCell::new();
        let usb = USB_DEVICE.init(usb_device);

        spawner.spawn(unwrap!(usb_device_task(usb)));

        defmt::info!("RTT logger ready");
        (ep_in, ep_out)
    };

    #[cfg(not(feature = "rtt"))]
    let (ep_in, ep_out) = {
        let (builder, ep_in, ep_out) = usb::usb_device(p.USBD);
        // Consumes the builder and spawns the USB device task.
        defmt_serial::init(&mut spawner, builder);
        defmt::info!("USB serial logger initialized");
        (ep_in, ep_out)
    };

    // MPSL is needed even without BLE, to coordinate flash access with the radio.
    info!("MPSL initialization");

    let mpsl_p =
        mpsl::Peripherals::new(p.RTC0, p.TIMER0, p.TEMP, p.PPI_CH19, p.PPI_CH30, p.PPI_CH31);
    let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        accuracy_ppm: mpsl::raw::MPSL_DEFAULT_CLOCK_ACCURACY_PPM as u16,
        skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
    };
    // Timeslots let nrf_mpsl::Flash schedule NVMC operations between radio events.
    static MPSL: StaticCell<mpsl::MultiprotocolServiceLayer> = StaticCell::new();
    static MPSL_SESSION: StaticCell<mpsl::SessionMem<1>> = StaticCell::new();
    let session_mem = MPSL_SESSION.init(mpsl::SessionMem::<1>::new());
    let mpsl = MPSL.init(
        mpsl::MultiprotocolServiceLayer::with_timeslots::<
            embassy_nrf::interrupt::typelevel::EGU0_SWI0,
            Irqs,
            1,
        >(mpsl_p, Irqs, lfclk_cfg, session_mem)
        .unwrap(),
    );
    spawner.spawn(unwrap!(mpsl_task(mpsl)));

    info!("SDC initialization");
    let sdc_p = sdc::Peripherals::new(
        p.PPI_CH17, p.PPI_CH18, p.PPI_CH20, p.PPI_CH21, p.PPI_CH22, p.PPI_CH23, p.PPI_CH24,
        p.PPI_CH25, p.PPI_CH26, p.PPI_CH27, p.PPI_CH28, p.PPI_CH29,
    );
    // SDC requires 72,328 bytes for six central links with this buffer configuration.
    const SDC_MEMORY_SIZE: usize = 72_328;
    static SDC_MEM: StaticCell<sdc::Mem<SDC_MEMORY_SIZE>> = StaticCell::new();
    let sdc_mem = SDC_MEM.init(sdc::Mem::new());

    use embassy_nrf::mode::Async as RngAsync;
    static RNG: StaticCell<rng::Rng<'static, RngAsync>> = StaticCell::new();
    let rng: &'static mut rng::Rng<'static, RngAsync> = RNG.init(rng::Rng::new(p.RNG, Irqs));
    let mut seed = [0u8; 32];
    rng.fill_bytes(&mut seed).await;
    let sdc = sdc::Builder::new()
        .unwrap()
        .support_scan()
        .support_ext_scan()
        .support_central()
        .support_ext_central()
        .support_phy_update_central()
        .support_le_2m_phy()
        .central_count(odyssey_protocol::accessory::TOTAL_CAPACITY as u8)
        .unwrap()
        .buffer_cfg(
            DefaultPacketPool::MTU as u16,
            DefaultPacketPool::MTU as u16,
            (odyssey_protocol::accessory::TOTAL_CAPACITY * 3) as u8,
            (odyssey_protocol::accessory::TOTAL_CAPACITY * 3) as u8,
        )
        .unwrap()
        .build(sdc_p, rng, mpsl, &mut *sdc_mem)
        .unwrap();

    // Six tracker CoCs plus headroom for all six links' ATT/signalling channels.
    info!("trouble-host initialization");
    static RESOURCES: StaticCell<
        trouble_host::HostResources<Pool, { odyssey_protocol::accessory::TOTAL_CAPACITY }, 24>,
    > = StaticCell::new();
    let resources = RESOURCES.init(trouble_host::HostResources::new());
    static STACK_CELL: StaticCell<trouble_host::Stack<'static, Controller, Pool>> =
        StaticCell::new();
    let mut host_seed_rng = ChaCha20Rng::from_seed(seed);
    let ficr = embassy_nrf::pac::FICR;
    let id_low = ficr.deviceid(0).read().to_le_bytes();
    let id_high = ficr.deviceid(1).read().to_le_bytes();
    let address = trouble_host::Address::random([
        id_low[0], id_low[1], id_low[2], id_low[3], id_high[0], id_high[1],
    ]);
    let stack = STACK_CELL.init(
        trouble_host::new(sdc, resources)
            .set_random_address(address)
            .set_random_generator_seed(&mut host_seed_rng),
    );

    // stack.build() waits until settings are up, so bonds can be preloaded first.

    red.set_high();

    // Flashes "blue" (LED2, P0.14) while pairing.
    let blue = embassy_nrf::gpio::Output::new(
        p.P0_14,
        embassy_nrf::gpio::Level::High,
        embassy_nrf::gpio::OutputDrive::Standard,
    );
    spawner.spawn(unwrap!(pairing::pairing_led_task(blue)));

    let flash = nrf_mpsl::Flash::take(mpsl, p.NVMC);
    let settings = storage::init_settings(&spawner, flash).await;

    let active_connections = ACTIVE_CONNECTIONS.init(ActiveConnections::new());

    // Preloaded bonds let reconnects encrypt without re-pairing. ClearBonds
    // purges them from the host through HOST_MGMT_CH so no stale LTK survives.
    {
        let bonds = settings.get_all_bonds().await;
        for opt in bonds.slots.iter() {
            if let Some(bd) = opt {
                let info = ble::security::info_from_bonddata(bd);
                let _ = stack.add_bond_information(info);
            }
        }
    }
    for bond in settings.get_accessory_bonds().await.slots.iter().flatten() {
        if stack
            .add_bond_information(ble::security::info_from_bonddata(bond))
            .is_err()
        {
            warn!("Failed to preload accessory bond");
        }
    }

    let host = stack.build();
    let (rx, control, tx) = host.runner.split();
    info!("Spawning host_rx_task");
    spawner.spawn(unwrap!(host_rx_task(rx)));
    info!("Spawning host_ctrl_task");
    spawner.spawn(unwrap!(host_ctrl_task(control)));
    info!("Spawning host_tx_task");
    spawner.spawn(unwrap!(host_tx_task(tx)));
    info!("Spawning host_mgmt_task");
    spawner.spawn(unwrap!(host_mgmt_task(stack, settings)));

    static DEVICE_PACKETS: StaticCell<DevicePacketChannel> = StaticCell::new();
    let device_packets = DEVICE_PACKETS.init(Channel::new());
    let (evt_ch, cmd_ch) = control::init();
    control::register(evt_ch, cmd_ch);

    use ble::central::DEVICE_QUEUES;
    let device_queues = DEVICE_QUEUES.init(ble::central::DeviceQueues::new());

    spawner.spawn(unwrap!(usb_rx_task(
        ep_out,
        device_queues,
        active_connections
    )));
    // Both TX tasks share ep_in through EP_MUTEX.
    let ep_mutex = EP_MUTEX.init(embassy_sync::mutex::Mutex::new(Some(ep_in)));
    spawner.spawn(unwrap!(host_responses_tx_task(ep_mutex)));
    spawner.spawn(unwrap!(device_packets_tx_task(ep_mutex, device_packets)));
    spawner.spawn(unwrap!(control_exec_task(settings)));
    spawner.spawn(unwrap!(ble::accessories::dispatcher(
        stack,
        settings,
        active_connections,
        spawner
    )));
    spawner.spawn(unwrap!(storage::accessory_bond_worker(settings)));
    info!("Starting BLE manager task");
    spawner.spawn(unwrap!(ble_task(
        device_packets,
        device_queues,
        settings,
        active_connections,
        stack,
        spawner,
    )));

    // Always-on active scan. Feeds the name cache and pairing detection.
    spawner.spawn(unwrap!(ble::pairing_scanner::background_scanner_task(
        stack
    )));
    spawner.spawn(unwrap!(ble::pairing_scanner::pairing_monitor_task(
        settings
    )));

    spawner.spawn(unwrap!(bond_storage_task(settings)));
}

#[embassy_executor::task]
async fn bond_storage_task(settings: &'static Settings) -> ! {
    use ble::security::{BOND_COMMANDS, BondCommand};
    use defmt::info;

    loop {
        let bond = match BOND_COMMANDS.receive().await {
            BondCommand::Store(bond) => bond,
            BondCommand::Clear => {
                // Stop every producer before dropping queued completions. No
                // store can interleave with the clears below: this task owns both.
                settings.scan_list_clear().await;
                ble::task_slots::stop_all().await;
                while BOND_COMMANDS.try_receive().is_ok() {}
                HOST_MGMT_CH.send(HostMgmtCmd::PurgeBonds).await;
                HOST_MGMT_DONE.wait().await;
                settings.bonds_clear().await;
                ble::central::clear_device_names();
                settings.clear_device_names().await;
                BONDS_CLEARED.signal(());
                continue;
            }
        };

        info!("Storing bond for device {:02x}...", bond.bd_addr);

        match settings.add_bond(bond).await {
            Ok(()) => {
                info!("Bond stored successfully for device {:02x}", bond.bd_addr);

                let _ = settings.add_scan_target(bond.bd_addr).await;

                // Prefer DEVICE_NAMES, which sees every adv report including scan
                // responses, over PAIRING_FOUND_NAME, which may come from the primary
                // ADV_IND before the scan response carrying the name arrived.
                let paired_name = {
                    let cached = crate::ble::central::get_cached_device_name(&bond.bd_addr);
                    let pairing = crate::ble::central::get_pairing_name(&bond.bd_addr);
                    info!(
                        "bond_storage: cached_name={:?} pairing_name={:?}",
                        cached.as_str(),
                        pairing.as_str()
                    );
                    if cached.is_empty() { pairing } else { cached }
                };
                info!("bond_storage: paired_name={:?}", paired_name.as_str());
                if !paired_name.is_empty() {
                    settings.set_device_name(bond.bd_addr, &paired_name).await;
                    info!("bond_storage: saved name to flash");
                } else {
                    info!("bond_storage: no name to save");
                }

                control::try_send_event(UsbMuxCtrlMsg::PairingResult(
                    odyssey_protocol::control::usb_mux::PairingOutcome::Ok(
                        odyssey_protocol::control::BondedDevice {
                            uuid: bond.bd_addr,
                            name: paired_name.into(),
                        },
                    ),
                ));

                pairing::cancel();
                info!("Pairing mode exited after successful bonding");
            }
            Err(()) => {
                defmt::error!("Failed to store bond for device {:02x}", bond.bd_addr);
                control::try_send_event(UsbMuxCtrlMsg::BondStoreError(BondStoreError::Full));
            }
        }
    }
}

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static mpsl::MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

#[cfg(feature = "rtt")]
#[embassy_executor::task]
async fn usb_device_task(usb: &'static mut embassy_usb::UsbDevice<'static, usb::UsbDriver>) -> ! {
    usb.run().await
}

#[embassy_executor::task]
async fn host_rx_task(mut rx: trouble_host::prelude::RxRunner<'static, Controller, Pool>) -> ! {
    defmt::info!("host_rx_task: starting");
    loop {
        match rx.run_with_handler(&HOST_HANDLER).await {
            Ok(_) => defmt::warn!("host_rx_task: run() completed unexpectedly"),
            Err(_) => defmt::error!("host_rx_task: encountered an error"),
        }
    }
}

#[embassy_executor::task]
async fn host_ctrl_task(
    mut ctrl: trouble_host::prelude::ControlRunner<'static, Controller, Pool>,
) -> ! {
    defmt::info!("host_ctrl_task: starting");
    loop {
        match ctrl.run().await {
            Ok(_) => defmt::warn!("host_ctrl_task: run() completed unexpectedly"),
            Err(_) => defmt::error!("host_ctrl_task: encountered an error"),
        }
    }
}

#[embassy_executor::task]
async fn host_mgmt_task(
    stack: &'static trouble_host::Stack<'static, Controller, Pool>,
    settings: &'static Settings,
) -> ! {
    defmt::info!("host_mgmt_task: starting");
    loop {
        match HOST_MGMT_CH.receive().await {
            HostMgmtCmd::PurgeBonds => {
                // Include bonds paired just before cancellation, even if their
                // persistence notification was still queued or was cancelled.
                let bonds = stack.get_bond_information();
                defmt::info!("host_mgmt: purging {} bond(s) from host DB", bonds.len());
                for info in bonds {
                    let _ = stack.remove_bond_information(info.identity);
                }
                for bond in settings.get_accessory_bonds().await.slots.iter().flatten() {
                    let _ = stack.add_bond_information(ble::security::info_from_bonddata(bond));
                }
                HOST_MGMT_DONE.signal(());
            }
            HostMgmtCmd::AddBond(bd) => {
                let info = ble::security::info_from_bonddata(&bd);
                let result = stack.add_bond_information(info).map_err(|_| ());
                HOST_MGMT_ADD_RESULT.signal(result);
            }
        }
    }
}

#[embassy_executor::task]
async fn host_tx_task(mut tx: trouble_host::prelude::TxRunner<'static, Controller, Pool>) -> ! {
    defmt::info!("host_tx_task: starting");
    loop {
        match tx.run().await {
            Ok(_) => defmt::warn!("host_tx_task: run() completed unexpectedly"),
            Err(_) => defmt::error!("host_tx_task: encountered an error"),
        }
    }
}

#[embassy_executor::task]
async fn ble_task(
    device_packets: &'static DevicePacketChannel,
    device_queues: &'static ble::central::DeviceQueues,
    settings: &'static Settings,
    active_connections: &'static ActiveConnections,
    stack: &'static trouble_host::Stack<'static, Controller, Pool>,
    spawner: embassy_executor::Spawner,
) -> ! {
    defmt::info!("ble_task: Creating BleManager");
    let mut ble_manager = BleManager::new(
        device_packets,
        device_queues,
        settings,
        active_connections,
        stack,
        spawner,
    );
    defmt::info!("ble_task: Starting BleManager run loop");
    ble_manager.run().await
}

// Reads host commands from the bulk OUT endpoint, re-arming after every
// deconfigure. A deconfigure or bus reset disables the endpoint, which ends
// the pending read with `EndpointError::Disabled`.
#[embassy_executor::task]
async fn usb_rx_task(
    mut ep_out: embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::Out>,
    device_queues: &'static ble::central::DeviceQueues,
    active_connections: &'static ActiveConnections,
) {
    loop {
        info!("USB RX task: waiting for USB configuration");
        let mut cfg = crate::usb::usb_config_receiver();
        if cfg.try_get().unwrap_or(false) {
        } else {
            cfg.changed_and(|&v| v).await;
        }

        let mut read_buf = [0u8; 512];

        loop {
            match EndpointOut::read_transfer(&mut ep_out, &mut read_buf).await {
                Ok(n) => {
                    defmt::trace!("USB RX: received {} bytes", n);
                    if let Err(_e) =
                        handle_usb_rx(&read_buf[..n], device_queues, active_connections).await
                    {
                        error!("Error handling USB RX ({} bytes)", n);
                    }
                }
                Err(EndpointError::Disabled) => {
                    info!("USB RX: disabled, waiting for reconnection");
                    break;
                }
                Err(_e) => {
                    warn!("USB RX: read error");
                }
            }
        }
    }
}

// The bulk IN endpoint, shared by both USB TX tasks.
static EP_MUTEX: StaticCell<
    embassy_sync::mutex::Mutex<
        ThreadModeRawMutex,
        Option<embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>>,
    >,
> = StaticCell::new();

// Sends HOST_RESPONSES (RequestDevices, ReadVersion, ...) on the IN endpoint.
#[embassy_executor::task]
async fn host_responses_tx_task(
    ep_mutex: &'static embassy_sync::mutex::Mutex<
        ThreadModeRawMutex,
        Option<embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>>,
    >,
) {
    let mut write_buf = [0u8; 256];

    loop {
        let mut cfg = crate::usb::usb_config_receiver();
        if cfg.try_get().unwrap_or(false) {
        } else {
            cfg.changed_and(|&v| v).await;
        }
        info!("USB TX host task: configured");

        loop {
            let generation = ble::accessories::EPOCH.load(Ordering::Acquire);
            // Bounded wait so a deconfigure is noticed while idle.
            let response = match embassy_time::with_timeout(
                embassy_time::Duration::from_millis(250),
                embassy_futures::select::select(
                    ble::accessories::TX_RESET.wait(),
                    HOST_RESPONSES.receive(),
                ),
            )
            .await
            {
                Ok(embassy_futures::select::Either::First(())) => break,
                Ok(embassy_futures::select::Either::Second(msg)) => msg,
                Err(_) => {
                    if let Some(v) = cfg.try_get() {
                        if !v {
                            break;
                        }
                    }
                    continue;
                }
            };

            // Hold the lock across the write. The other task then waits for the
            // endpoint instead of finding None and dropping its message.
            let mut ep_guard = match embassy_futures::select::select(
                ble::accessories::TX_RESET.wait(),
                ep_mutex.lock(),
            )
            .await
            {
                embassy_futures::select::Either::First(()) => break,
                embassy_futures::select::Either::Second(guard) => guard,
            };
            if ble::accessories::EPOCH.load(Ordering::Acquire) != generation {
                continue;
            }
            if let Some(ref mut endpoint) = *ep_guard {
                let result = send_mux_msg(&response, &mut write_buf, endpoint).await;
                drop(ep_guard);
                if result.is_err() {
                    warn!("USB TX: error sending host response");
                    break;
                }
            } else {
                drop(ep_guard);
                warn!("USB TX host: endpoint not initialized");
            }
        }
        warn!("USB TX host task: deconfigured");
    }
}
// Forwards packets from BLE devices to the IN endpoint.
#[embassy_executor::task]
async fn device_packets_tx_task(
    ep_mutex: &'static embassy_sync::mutex::Mutex<
        ThreadModeRawMutex,
        Option<embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>>,
    >,
    device_packets: &'static DevicePacketChannel,
) {
    let mut write_buf = [0u8; 256];

    loop {
        let mut cfg = crate::usb::usb_config_receiver();
        if cfg.try_get().unwrap_or(false) {
        } else {
            cfg.changed_and(|&v| v).await;
        }
        info!("USB TX data task: configured");

        let mut idle_count: u32 = 0;
        let mut tx_count: u32 = 0;
        loop {
            // Bounded wait so a deconfigure is noticed while idle.
            let device_pkt = match embassy_time::with_timeout(
                embassy_time::Duration::from_millis(250),
                device_packets.receive(),
            )
            .await
            {
                Ok(pkt) => pkt,
                Err(_) => {
                    idle_count += 1;
                    // Log every 20 timeouts, about 5 s of idle.
                    if idle_count % 20 == 0 {
                        info!("USB TX data: idle {}s, sent {}", idle_count / 4, tx_count);
                    }
                    if let Some(v) = cfg.try_get() {
                        if !v {
                            break;
                        }
                    }
                    continue;
                }
            };
            idle_count = 0;
            let msg = MuxMsg::DevicePacket(device_pkt);

            // Hold the lock across the write. The other task then waits for the
            // endpoint instead of finding None and dropping its message.
            let mut ep_guard = ep_mutex.lock().await;
            if let Some(ref mut endpoint) = *ep_guard {
                let result = send_mux_msg(&msg, &mut write_buf, endpoint).await;
                drop(ep_guard);
                if result.is_ok() {
                    tx_count += 1;
                } else {
                    // A real driver error. The endpoint is likely disabled or stalled.
                    warn!("USB TX data: write failed after {} sent", tx_count);
                    break;
                }
            } else {
                drop(ep_guard);
                warn!("USB TX data: endpoint not initialized");
            }
        }
        warn!("USB TX data task: deconfigured");
    }
}

async fn handle_usb_rx(
    data: &[u8],
    device_queues: &ble::central::DeviceQueues,
    active_connections: &ActiveConnections,
) -> Result<(), ()> {
    defmt::trace!("handle_usb_rx: {} bytes", data.len());

    // One USB transfer can carry several postcard messages back to back.
    let mut remaining = data;
    while !remaining.is_empty() {
        let (msg, rest): (MuxMsg, &[u8]) = postcard::take_from_bytes(remaining).map_err(|e| {
            error!(
                "Failed to deserialize USB packet ({} bytes remaining of {} total): {:?}",
                remaining.len(),
                data.len(),
                defmt::Debug2Format(&e)
            );
            ()
        })?;
        remaining = rest;

        handle_usb_msg(msg, device_queues, active_connections).await;
    }

    Ok(())
}

async fn handle_usb_msg(
    msg: MuxMsg,
    device_queues: &ble::central::DeviceQueues,
    active_connections: &ActiveConnections,
) {
    // Logging every message overflows RTT, so only config writes are logged.
    if let MuxMsg::SendTo(s) = &msg {
        if matches!(
            s.pkt.data,
            odyssey_protocol::PacketData::WriteConfig(_)
                | odyssey_protocol::PacketData::FlashSettings()
        ) {
            defmt::info!("RX: {:?}", defmt::Debug2Format(&s.pkt.data));
        }
    }

    match msg {
        MuxMsg::AccessoryRequest(request) => ble::accessories::submit(request).await,
        MuxMsg::RequestDevices() => {
            let devices = active_connections.get_all().await;
            let response = MuxMsg::DevicesSnapshot(devices.into());
            if HOST_RESPONSES.try_send(response).is_err() {
                warn!("HOST_RESPONSES full, dropped DevicesSnapshot");
            }
        }
        MuxMsg::SendTo(send_to_msg) => {
            let device_uuid = &send_to_msg.dev;
            defmt::trace!("SendTo message for device {:02x}", device_uuid);
            if let Some(queue) = device_queues.get_queue(device_uuid).await {
                let device_pkt = DevicePacket {
                    dev: send_to_msg.dev,
                    pkt: send_to_msg.pkt,
                };
                queue.send(device_pkt).await;
            } else {
                error!(
                    "No queue found for device {:02x} - device not connected",
                    device_uuid
                );
            }
        }
        MuxMsg::ReadVersion() => {
            let response = MuxMsg::ReadVersionResponse(Version::new(FIRMWARE_SEMVER));
            if HOST_RESPONSES.try_send(response).is_err() {
                warn!("HOST_RESPONSES full, dropped ReadVersionResponse");
            }
        }
        MuxMsg::SubscribeDeviceList() => {
            info!("Subscribing to device list changes");
            DEVICE_LIST_SUBSCRIBED.store(true, Ordering::Relaxed);
        }
        MuxMsg::UnsubscribeDeviceList() => {
            info!("Unsubscribing from device list changes");
            DEVICE_LIST_SUBSCRIBED.store(false, Ordering::Relaxed);
        }
        _ => {
            error!(
                "Unexpected message from host: {:?}",
                defmt::Debug2Format(&msg)
            );
        }
    }
}

async fn send_mux_msg(
    msg: &MuxMsg,
    write_buf: &mut [u8],
    ep_in: &mut embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>,
) -> Result<(), ()> {
    use embassy_usb::driver::EndpointIn;

    // Postcard, as ats_usb expects on this endpoint.
    let data = postcard::to_slice(msg, write_buf).map_err(|_| ())?;

    defmt::trace!("write_transfer start, {} bytes", data.len());
    let result = EndpointIn::write_transfer(ep_in, data, true).await;
    match result {
        Ok(()) => {
            defmt::trace!("write_transfer done");
            Ok(())
        }
        Err(e) => {
            warn!("USB TX write_transfer error: {:?}", defmt::Debug2Format(&e));
            Err(())
        }
    }
}

#[embassy_executor::task]
async fn control_exec_task(settings: &'static Settings) -> ! {
    // Drop stale events once at startup only. Clearing on every command would race
    // the host and lose responses such as ClearBondsResponse.
    control::clear_events();
    loop {
        use UsbMuxCtrlMsg as C;
        let cmd = control::recv_cmd().await;
        match cmd {
            C::ReadVersion() => {
                control::try_send_event(C::ReadVersionResponse(odyssey_protocol::Version::new(
                    FIRMWARE_SEMVER,
                )));
            }
            C::ListBonds() => {
                info!("CTL ListBonds received");
                let bonds = settings.get_all_bonds().await;
                let mut bonded = HVec::new();
                for opt in bonds.slots.iter() {
                    if let Some(bd) = opt {
                        // The adv cache tracks the name the device advertises now,
                        // including after a USB rename. Flash is the fallback.
                        let cached = crate::ble::central::get_cached_device_name(&bd.bd_addr);
                        let name = if !cached.is_empty() {
                            cached
                        } else {
                            settings.get_device_name(&bd.bd_addr).await
                        };
                        let _ = bonded.push(odyssey_protocol::control::BondedDevice {
                            uuid: bd.bd_addr,
                            name: name.into(),
                        });
                    }
                }
                for b in bonded.iter() {
                    info!("  bond {:02x} name={}", b.uuid, b.name.as_str());
                }
                control::try_send_event(C::ListBondsResponse(bonded.into()));
            }
            C::StartPairing(start) => {
                info!("CTL StartPairing received, timeout_ms={}", start.timeout_ms);
                let timeout = embassy_time::Duration::from_millis(start.timeout_ms as u64);
                ble::pairing_scanner::on_pairing_start();
                pairing::enter_with_timeout(timeout);
                control::try_send_event(C::StartPairingResponse());
            }
            C::CancelPairing() => {
                info!("CTL CancelPairing received");
                pairing::cancel();
                control::try_send_event(C::PairingResult(PairingOutcome::Err(
                    PairingError::Cancelled,
                )));
            }
            C::ClearBonds() => {
                info!("CTL ClearBonds received");
                // Stop pairing so its events cannot interleave with the clear.
                pairing::cancel();
                ble::security::BOND_COMMANDS
                    .send(ble::security::BondCommand::Clear)
                    .await;
                BONDS_CLEARED.wait().await;

                control::try_send_event(C::ClearBondsResponse(ClearBondsResult::Ok()));
            }
            C::AddBond(entry) => {
                use odyssey_protocol::control::{AddBondError, AddBondResult};
                info!("CTL AddBond received for {:02x}", entry.bd_addr);
                let bond: BondData = entry.into();
                match settings.add_bond(bond).await {
                    Ok(()) => {
                        let _ = settings.add_scan_target(bond.bd_addr).await;
                        HOST_MGMT_CH.send(HostMgmtCmd::AddBond(bond)).await;
                        let host_result = HOST_MGMT_ADD_RESULT.wait().await;
                        if host_result.is_err() {
                            warn!("AddBond: failed to register bond in host DB");
                        }
                        control::try_send_event(C::AddBondResponse(AddBondResult::Ok()));
                    }
                    Err(()) => {
                        control::try_send_event(C::AddBondResponse(AddBondResult::Err(
                            AddBondError::Full,
                        )));
                    }
                }
            }
            C::UpdateBondName { uuid, name } => {
                info!("CTL UpdateBondName {:02x} name={}", uuid, name.as_str());
                settings.set_device_name(uuid, &name.0).await;
                ble::central::update_device_name(uuid, name.0);
            }
            C::EnterBootloader() => {
                info!("CTL EnterBootloader received");
                // Let the control transfer's status stage finish before the dongle leaves the bus.
                embassy_time::Timer::after_millis(50).await;
                // nRF5 SDK bootloaders (Fanstel's, Laird's) stay in DFU mode after a
                // reset with GPREGRET set to BOOTLOADER_DFU_START.
                embassy_nrf::pac::POWER
                    .gpregret()
                    .write(|w| w.set_gpregret(0xB1));
                cortex_m::peripheral::SCB::sys_reset();
            }
            _ => {}
        }
    }
}
