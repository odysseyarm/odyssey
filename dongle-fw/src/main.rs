#![no_main]
#![no_std]

mod ble;
mod control;
#[cfg(not(feature = "rtt"))]
mod defmt_serial;
mod pairing;
mod storage;
mod usb;

#[cfg(feature = "rtt")]
use {defmt_rtt as _, panic_probe as _};

use core::sync::atomic::{AtomicBool, Ordering};
use cortex_m_rt::{ExceptionFrame, exception};
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_nrf::{bind_interrupts, peripherals, usb as nrf_usb};
use embassy_nrf::{peripherals::RNG as NRF_RNG, rng};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal as SyncSignal;

// Channel for sending host responses (RequestDevices, ReadVersion, etc.)
type HostResponseChannel = Channel<ThreadModeRawMutex, MuxMsg, 4>;
pub static HOST_RESPONSES: HostResponseChannel = Channel::new();

// Device list subscription flag
pub static DEVICE_LIST_SUBSCRIBED: AtomicBool = AtomicBool::new(false);
use embassy_time::Timer;
use embassy_usb::driver::{EndpointError, EndpointOut};
// Using trouble-host + nrf-sdc
use protodongers::control::usb_mux::{BondStoreError, PairingError, UsbMuxCtrlMsg, UsbMuxVersion};
use protodongers::mux::{DevicePacket, MAX_DEVICES, MuxMsg, Version};

use static_cell::StaticCell;

use nrf_sdc::{self as sdc, mpsl};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use trouble_host::PacketPool;
use trouble_host::prelude::DefaultPacketPool;

type Controller = nrf_sdc::SoftdeviceController<'static>;
type Pool = DefaultPacketPool;

use crate::ble::{ACTIVE_CONNECTIONS, ActiveConnections, BleManager, DevicePacketChannel};
use crate::storage::Settings;

use crate::storage::BondData;

use heapless::Vec as HVec;

// Host management channel to handle bond purge requests while keeping
// preloading at boot. ClearBonds will send to this channel and wait for
// completion before responding.
enum HostMgmtCmd {
    PurgeBonds(HVec<BondData, { MAX_DEVICES }>),
}

static HOST_MGMT_CH: Channel<ThreadModeRawMutex, HostMgmtCmd, 2> = Channel::new();
static HOST_MGMT_DONE: SyncSignal<ThreadModeRawMutex, ()> = SyncSignal::new();
// Capture scan reports during pairing using a host event handler
struct HostEventHandler;
impl trouble_host::prelude::EventHandler for HostEventHandler {
    fn on_adv_reports(&self, reports: trouble_host::scan::LeAdvReportsIter) {
        if crate::pairing::is_active() {
            for r in reports {
                if let Ok(rep) = r {
                    // During pairing, accept MSD in either primary ADV or SCAN_RSP.
                    if crate::ble::central::adv_matches_pairing(rep.data) {
                        let raw = rep.addr.raw();
                        let mut addr = [0u8; 6];
                        addr.copy_from_slice(raw);
                        defmt::info!("pairing: adv/scan match (LEGACY) {:02x}", addr);
                        crate::ble::central::report_scan_address(addr);
                        break;
                    }
                }
            }
        }
    }
    fn on_ext_adv_reports(&self, reports: trouble_host::scan::LeExtAdvReportsIter) {
        if crate::pairing::is_active() {
            for r in reports {
                if let Ok(rep) = r {
                    // Accept any extended advert that contains our MSD.
                    if crate::ble::central::adv_matches_pairing(rep.data) {
                        let raw = rep.addr.raw();
                        let mut addr = [0u8; 6];
                        addr.copy_from_slice(raw);
                        defmt::info!("pairing: adv match (EXT) {:02x}", addr);
                        crate::ble::central::report_scan_address(addr);
                        break;
                    }
                }
            }
        }
    }
}
static HOST_HANDLER: HostEventHandler = HostEventHandler;

bind_interrupts!(pub struct Irqs {
    // CLOCK_POWER for both USB VBUS detect and MPSL
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler, mpsl::ClockInterruptHandler;

    // MPSL interrupt handlers
    EGU0_SWI0 => mpsl::LowPrioInterruptHandler;
    RADIO => mpsl::HighPrioInterruptHandler;
    TIMER0 => mpsl::HighPrioInterruptHandler;
    RTC0 => mpsl::HighPrioInterruptHandler;

    RNG => rng::InterruptHandler<NRF_RNG>;
    USBD => nrf_usb::InterruptHandler<peripherals::USBD>;
});

// Hardware VBUS detect is used; no SoftwareVbusDetect needed.

const VERSION: Version = Version {
    protocol_semver: [0, 1, 0],
    firmware_semver: [0, 1, 0],
};
const USB_MUX_VERSION: UsbMuxVersion = UsbMuxVersion {
    protocol_semver: [0, 1, 0],
    firmware_semver: [0, 1, 0],
};

#[embassy_executor::main]
async fn main(mut spawner: Spawner) {
    defmt::info!("Booted");
    let p = embassy_nrf::init(Default::default());

    embassy_time::Timer::after_millis(2000).await;

    // nRF52840-DK LEDs: P0.13, P0.14, P0.15, P0.16 (all white)
    // Using P0.13 as "red" (status), P0.14 as "blue" (pairing)
    let red = p.P0_08;

    let mut red = embassy_nrf::gpio::Output::new(
        red,
        embassy_nrf::gpio::Level::High,
        embassy_nrf::gpio::OutputDrive::Standard,
    );

    red.set_low();

    // Default interrupt priorities are fine; no overrides needed.

    // Flash blue LED while pairing is active

    // Initialize USB device and logging
    #[cfg(feature = "rtt")]
    let (ep_in, ep_out, usb_signal) = {
        use static_cell::StaticCell;

        // When using RTT, build USB device without serial logger
        let (builder, ep_in, ep_out, usb_signal) = usb::usb_device(p.USBD);
        let usb_device = builder.build();

        static USB_DEVICE: StaticCell<embassy_usb::UsbDevice<'static, usb::UsbDriver>> =
            StaticCell::new();
        let usb = USB_DEVICE.init(usb_device);

        // Spawn USB device task
        spawner.must_spawn(usb_device_task(usb));

        defmt::info!("RTT logger ready");
        (ep_in, ep_out, usb_signal)
    };

    #[cfg(not(feature = "rtt"))]
    let (ep_in, ep_out, usb_signal) = {
        let (builder, ep_in, ep_out, usb_signal) = usb::usb_device(p.USBD);
        // Initialize USB serial logger (consumes builder and spawns USB device task)
        defmt_serial::init(&mut spawner, builder);
        defmt::info!("USB serial logger initialized");
        (ep_in, ep_out, usb_signal)
    };

    for _ in 0..3 {
        defmt::info!("Hello, World!");
        Timer::after_secs(1).await;
    }

    // Initialize nrf-mpsl (required even without BLE for flash access coordination)
    info!("MPSL initialization");

    // MPSL init
    let mpsl_p =
        mpsl::Peripherals::new(p.RTC0, p.TIMER0, p.TEMP, p.PPI_CH19, p.PPI_CH30, p.PPI_CH31);
    let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        accuracy_ppm: mpsl::raw::MPSL_DEFAULT_CLOCK_ACCURACY_PPM as u16,
        skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
    };
    // Initialize MPSL with timeslots so nrf_mpsl::Flash can schedule NVMC operations safely.
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
    spawner.must_spawn(mpsl_task(mpsl));

    // SDC init
    info!("SDC initialization");
    let sdc_p = sdc::Peripherals::new(
        p.PPI_CH17, p.PPI_CH18, p.PPI_CH20, p.PPI_CH21, p.PPI_CH22, p.PPI_CH23, p.PPI_CH24,
        p.PPI_CH25, p.PPI_CH26, p.PPI_CH27, p.PPI_CH28, p.PPI_CH29,
    );
    // Match example scanner minimal memory; adjust if needed by features
    static SDC_MEM: StaticCell<sdc::Mem<5408>> = StaticCell::new();
    let sdc_mem = SDC_MEM.init(sdc::Mem::new());
    use embassy_nrf::mode::Async as RngAsync;
    static RNG: StaticCell<rng::Rng<'static, RngAsync>> = StaticCell::new();
    let rng: &'static mut rng::Rng<'static, RngAsync> = RNG.init(rng::Rng::new(p.RNG, Irqs));
    let mut seed = [0u8; 32];
    rng.fill_bytes(&mut seed).await;
    let sdc = sdc::Builder::new()
        .unwrap()
        .support_scan()
        .unwrap()
        .support_ext_scan()
        .unwrap()
        .support_central()
        .unwrap()
        .support_ext_central()
        .unwrap()
        .support_phy_update_central()
        .unwrap()
        .support_le_2m_phy()
        .unwrap()
        .central_count(1)
        .unwrap()
        .buffer_cfg(
            DefaultPacketPool::MTU as u16,
            DefaultPacketPool::MTU as u16,
            3,
            3,
        )
        .unwrap()
        .build(sdc_p, rng, mpsl, &mut *sdc_mem)
        .unwrap();

    // Host resources and stack
    info!("trouble-host initialization");
    // Need 2 L2CAP channels per device (control + data), so 16 channels for 8 devices
    static RESOURCES: StaticCell<trouble_host::HostResources<Pool, { MAX_DEVICES }, 16>> =
        StaticCell::new();
    let resources = RESOURCES.init(trouble_host::HostResources::new());
    static STACK_CELL: StaticCell<trouble_host::Stack<'static, Controller, Pool>> =
        StaticCell::new();
    let mut host_seed_rng = ChaCha20Rng::from_seed(seed);
    // Set a static random address like the examples; replace with a stable ID if desired.
    let address = trouble_host::Address::random([0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03]);
    let stack = STACK_CELL.init(
        trouble_host::new(sdc, resources)
            .set_random_address(address)
            .set_random_generator_seed(&mut host_seed_rng),
    );

    // Defer stack.build() until after settings are initialized so we can preload bonds.

    red.set_high();

    // Start pairing LED task (flash "blue" while pairing) - using LED2 (P0.14)
    let blue = embassy_nrf::gpio::Output::new(
        p.P0_14,
        embassy_nrf::gpio::Level::High,
        embassy_nrf::gpio::OutputDrive::Standard,
    );
    spawner.must_spawn(pairing::pairing_led_task(blue));

    // Initialize settings using MPSL-coordinated flash access
    let flash = nrf_mpsl::Flash::take(mpsl, p.NVMC);
    let settings = storage::init_settings(&spawner, flash).await;

    // Initialize active connections tracker
    let active_connections = ACTIVE_CONNECTIONS.init(ActiveConnections::new());

    // Load bonds into cache for reconnection
    ble::security::load_bond_cache(settings).await;

    // Preload bonds into the host at runtime for convenient auto-encryption on reconnects.
    // Runtime ClearBonds will also purge the host DB via HOST_MGMT_CH to avoid stale LTKs.
    {
        let bonds = settings.get_all_bonds().await;
        for opt in bonds.slots.iter() {
            if let Some(bd) = opt {
                let info = ble::security::info_from_bonddata(bd);
                let _ = stack.add_bond_information(info);
            }
        }
    }

    // Now build the host and start its tasks
    let host = stack.build();
    let (rx, control, tx) = host.runner.split();
    spawner.must_spawn(host_rx_task(rx));
    spawner.must_spawn(host_ctrl_task(control));
    spawner.must_spawn(host_tx_task(tx));
    spawner.must_spawn(host_mgmt_task(stack));

    // Initialize channels for BLE <-> USB communication
    static DEVICE_PACKETS: StaticCell<DevicePacketChannel> = StaticCell::new();
    let device_packets = DEVICE_PACKETS.init(Channel::new());
    let (evt_ch, cmd_ch) = control::init();
    control::register(evt_ch, cmd_ch);

    // Initialize per-device queue registry
    use ble::central::DEVICE_QUEUES;
    let device_queues = DEVICE_QUEUES.init(ble::central::DeviceQueues::new());

    // Store global references for use in control task
    ble::central::set_global_refs(active_connections, device_queues).await;

    spawner.must_spawn(usb_rx_task(ep_out, device_queues, active_connections));
    spawner.must_spawn(usb_tx_task(ep_in, device_packets, usb_signal));
    spawner.must_spawn(control_exec_task(settings));
    // Start BLE manager (central)
    info!("Starting BLE manager task");
    spawner.must_spawn(ble_task(
        device_packets,
        device_queues,
        settings,
        active_connections,
        stack,
        host.central,
    ));
    spawner.must_spawn(bond_storage_task(settings));
}

// Hardware VBUS detect is used; no SoftDevice VBUS forwarding needed.

#[embassy_executor::task]
async fn bond_storage_task(settings: &'static Settings) -> ! {
    use ble::security::BOND_TO_STORE;
    use defmt::info;

    loop {
        // Wait for a bond to be stored
        let bond = BOND_TO_STORE.wait().await;

        info!("Storing bond for device {:02x}...", bond.bd_addr);

        // Store the bond to flash
        match settings.add_bond(bond).await {
            Ok(()) => {
                info!("Bond stored successfully for device {:02x}", bond.bd_addr);

                // Add to runtime scan targets so reconnection works without reboot
                let _ = settings.add_scan_target(bond.bd_addr).await;

                // Update the in-memory cache for fast lookup on reconnection
                ble::security::update_bond_cache(bond).await;

                // Send pairing success to USB host
                control::try_send_event(UsbMuxCtrlMsg::PairingResult(Ok(bond.bd_addr)));

                // Exit pairing mode now that we've successfully bonded
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

// Type aliases used across tasks

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
async fn host_mgmt_task(stack: &'static trouble_host::Stack<'static, Controller, Pool>) -> ! {
    defmt::info!("host_mgmt_task: starting");
    loop {
        match HOST_MGMT_CH.receive().await {
            HostMgmtCmd::PurgeBonds(list) => {
                defmt::info!("host_mgmt: purging {} bond(s) from host DB", list.len());
                for bd in list.iter() {
                    let info = ble::security::info_from_bonddata(bd);
                    let _ = stack.remove_bond_information(info.identity);
                }
                HOST_MGMT_DONE.signal(());
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
    central: trouble_host::prelude::Central<'static, Controller, Pool>,
) -> ! {
    defmt::info!("ble_task: Creating BleManager");
    let mut ble_manager = BleManager::<Controller, Pool>::new(
        device_packets,
        device_queues,
        settings,
        active_connections,
        stack,
        central,
    );
    defmt::info!("ble_task: Starting BleManager run loop");
    ble_manager.run().await
}

// USB RX task: receives commands from host
#[embassy_executor::task]
async fn usb_rx_task(
    mut ep_out: embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::Out>,
    device_queues: &'static ble::central::DeviceQueues,
    active_connections: &'static ActiveConnections,
) {
    loop {
        info!("USB RX task: waiting for USB configuration");

        let mut read_buf = [0u8; 512];

        loop {
            match EndpointOut::read_transfer(&mut ep_out, &mut read_buf).await {
                Ok(n) => {
                    info!("USB RX: received {} bytes", n);
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

            if pairing::take_timeout() {
                // Emit control-plane timeout event
                control::try_send_event(UsbMuxCtrlMsg::PairingResult(Err(PairingError::Timeout)));
            }
        }
    }
}

// USB TX task: sends device packets to host
// Static mutex shared by USB TX subtasks
static EP_MUTEX: StaticCell<
    embassy_sync::mutex::Mutex<
        ThreadModeRawMutex,
        Option<embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>>,
    >,
> = StaticCell::new();

#[embassy_executor::task]
async fn usb_tx_task(
    ep_in: embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>,
    device_packets: &'static DevicePacketChannel,
    usb_signal: &'static embassy_sync::signal::Signal<ThreadModeRawMutex, bool>,
) {
    // Initialize the mutex once (outside the loop since it's a StaticCell)
    let ep_mutex = EP_MUTEX.init(embassy_sync::mutex::Mutex::new(Some(ep_in)));

    loop {
        // Wait for USB to be configured
        loop {
            if usb_signal.wait().await {
                break; // USB is configured
            }
        }
        info!("USB TX task: USB configured, starting");

        // Spawn two independent tasks: one for host responses, one for device packets
        let host_task = host_responses_tx_task(ep_mutex);
        let device_task = device_packets_tx_task(ep_mutex, device_packets);

        // Run both tasks concurrently - neither will be cancelled
        use embassy_futures::join::join;
        let _ = join(host_task, device_task).await;

        warn!("USB TX: tasks ended, waiting for reconfiguration");
        // Loop will restart and wait for configuration again
    }
}

// Sends host responses (RequestDevices, ReadVersion, etc.) to USB endpoint
async fn host_responses_tx_task(
    ep_mutex: &'static embassy_sync::mutex::Mutex<
        ThreadModeRawMutex,
        Option<embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>>,
    >,
) {
    let mut write_buf = [0u8; 256];

    loop {
        let response = HOST_RESPONSES.receive().await;

        let mut ep_opt = ep_mutex.lock().await;
        if let Some(ep) = ep_opt.as_mut() {
            if send_mux_msg(&response, &mut write_buf, ep).await.is_err() {
                warn!("USB TX: error sending host response");
                break;
            }
        }
    }
}

// Sends device packets to USB endpoint
async fn device_packets_tx_task(
    ep_mutex: &'static embassy_sync::mutex::Mutex<
        ThreadModeRawMutex,
        Option<embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>>,
    >,
    device_packets: &'static DevicePacketChannel,
) {
    let mut write_buf = [0u8; 256];

    loop {
        let device_pkt = device_packets.receive().await;
        let msg = MuxMsg::DevicePacket(device_pkt);

        let mut ep_opt = ep_mutex.lock().await;
        if let Some(ep) = ep_opt.as_mut() {
            if send_mux_msg(&msg, &mut write_buf, ep).await.is_err() {
                warn!("USB TX: error sending device packet");
                break;
            }
        }
    }
}

async fn handle_usb_rx(
    data: &[u8],
    device_queues: &ble::central::DeviceQueues,
    active_connections: &ActiveConnections,
) -> Result<(), ()> {
    // Decode using postcard (standard for USB in ats_usb)
    let msg: MuxMsg = postcard::from_bytes(data).map_err(|e| {
        error!(
            "Failed to deserialize USB packet ({} bytes): {:?}",
            data.len(),
            defmt::Debug2Format(&e)
        );
        ()
    })?;

    match msg {
        MuxMsg::RequestDevices => {
            let devices = active_connections.get_all().await;
            let response = MuxMsg::DevicesSnapshot(devices);
            HOST_RESPONSES.send(response).await;
        }
        MuxMsg::SendTo(send_to_msg) => {
            // Route message to the specific device's queue
            let device_uuid = &send_to_msg.dev;
            info!("SendTo message for device {:02x}", device_uuid);
            if let Some(queue) = device_queues.get_queue(device_uuid).await {
                let device_pkt = DevicePacket {
                    dev: send_to_msg.dev,
                    pkt: send_to_msg.pkt,
                };
                // Use try_send to avoid blocking if queue is full (BLE task not draining fast enough)
                match queue.try_send(device_pkt) {
                    Ok(()) => {
                        info!("Packet queued successfully for device {:02x}", device_uuid);
                    }
                    Err(_) => {
                        warn!("Device queue full for {:02x}, packet dropped", device_uuid);
                    }
                }
            } else {
                error!(
                    "No queue found for device {:02x} - device not connected",
                    device_uuid
                );
            }
        }
        MuxMsg::ReadVersion() => {
            let response = MuxMsg::ReadVersionResponse(VERSION);
            HOST_RESPONSES.send(response).await;
        }
        MuxMsg::SubscribeDeviceList => {
            info!("Subscribing to device list changes");
            DEVICE_LIST_SUBSCRIBED.store(true, Ordering::Relaxed);
        }
        MuxMsg::UnsubscribeDeviceList => {
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

    Ok(())
}

async fn send_mux_msg(
    msg: &MuxMsg,
    write_buf: &mut [u8],
    ep_in: &mut embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>,
) -> Result<(), ()> {
    use embassy_usb::driver::EndpointIn;

    // Encode using postcard (standard for USB in ats_usb)
    let data = postcard::to_slice(msg, write_buf).map_err(|_| ())?;

    // Use write_transfer to handle large packets automatically
    EndpointIn::write_transfer(ep_in, data, true)
        .await
        .map_err(|_| ())?;
    Ok(())
}

#[embassy_executor::task]
async fn control_exec_task(settings: &'static Settings) -> ! {
    loop {
        use UsbMuxCtrlMsg as C;
        let cmd = control::recv_cmd().await;
        match cmd {
            C::ReadVersion() => control::try_send_event(C::ReadVersionResponse(USB_MUX_VERSION)),
            C::StartPairing(start) => {
                info!("CTL StartPairing received, timeout_ms={}", start.timeout_ms);
                let timeout = embassy_time::Duration::from_millis(start.timeout_ms as u64);
                pairing::enter_with_timeout(timeout);
                control::try_send_event(C::StartPairingResponse);
            }
            C::CancelPairing => {
                info!("CTL CancelPairing received");
                pairing::cancel();
                control::try_send_event(C::PairingResult(Err(PairingError::Cancelled)));
            }
            C::ClearBonds => {
                info!("CTL ClearBonds received");
                // Capture current bonds to purge from host DB
                let bonds = settings.get_all_bonds().await;
                let mut to_purge: HVec<BondData, { MAX_DEVICES }> = HVec::new();
                for opt in bonds.slots.iter() {
                    if let Some(bd) = opt {
                        let _ = to_purge.push(*bd);
                    }
                }
                // Ask host task to purge; wait briefly for completion
                HOST_MGMT_CH.send(HostMgmtCmd::PurgeBonds(to_purge)).await;
                let _ = embassy_time::with_timeout(
                    embassy_time::Duration::from_millis(500),
                    HOST_MGMT_DONE.wait(),
                )
                .await;
                // Now clear persisted + runtime state
                settings.bonds_clear().await;
                settings.scan_list_clear().await;
                ble::security::clear_bond_cache().await;

                // Disconnect all active BLE connections
                if let (Some(active_connections), Some(device_queues)) = (
                    ble::central::get_active_connections().await,
                    ble::central::get_device_queues().await,
                ) {
                    let active_devs = active_connections.get_all().await;
                    for dev in active_devs.iter() {
                        info!(
                            "Disconnecting active connection to {:02x} due to bond clear",
                            dev
                        );
                        active_connections.remove(dev).await;
                        device_queues.unregister(dev).await;
                    }
                }

                // Signal central loop to restart immediately (cancel any in-flight operations)
                ble::central::RESTART_CENTRAL.signal(());

                control::try_send_event(C::ClearBondsResponse(Ok(())));
            }
            _ => {}
        }
    }
}

#[exception]
unsafe fn HardFault(_ef: &ExceptionFrame) -> ! {
    // nRF52840-DK: Turn on LED1 (P0.13) to indicate hardfault
    let red = unsafe { embassy_nrf::peripherals::P0_08::steal() };
    let blue = unsafe { embassy_nrf::peripherals::P0_12::steal() };
    let _blue = embassy_nrf::gpio::Output::new(
        blue,
        embassy_nrf::gpio::Level::High,
        embassy_nrf::gpio::OutputDrive::Standard,
    );
    let _red = embassy_nrf::gpio::Output::new(
        red,
        embassy_nrf::gpio::Level::Low,
        embassy_nrf::gpio::OutputDrive::Standard,
    );
    loop {}
}
