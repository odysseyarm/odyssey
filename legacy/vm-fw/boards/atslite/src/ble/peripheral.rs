use embassy_futures::join::join;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use protodongers::control::device::TransportMode;
use trouble_host::prelude::*;

use crate::transport_mode;
use common::settings;

/// Max number of connections
const CONNECTIONS_MAX: usize = 1;

/// Max number of L2CAP channels (need 2: control + data).
const L2CAP_CHANNELS_MAX: usize = 2;

type Pool = DefaultPacketPool;

// GATT Server definition
#[gatt_server]
struct Server {}

/// Run the BLE stack.
pub async fn run<C>(controller: C)
where
    C: Controller,
{
    defmt::info!("[adv] advertising task starting");
    let address = Address::random(defmt::unwrap!(crate::utils::device_id()[0..6].try_into()));

    let mut resources: HostResources<Pool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();

    let stack = trouble_host::new(controller, &mut resources).set_random_address(address);

    // Safety: init_settings is called before BLE task is spawned
    let settings = unsafe { settings::get_settings() };

    // Restore any persisted bond into the host so reconnections encrypt without re-pairing.
    let mut bond_stored = if let Some(bond_info) = settings.ble_bond.to_bond_info() {
        match stack.add_bond_information(bond_info) {
            Ok(()) => {
                defmt::info!("Restored bond from settings into host");
                true
            }
            Err(e) => {
                defmt::warn!(
                    "Failed to restore bond into host: {:?}",
                    defmt::Debug2Format(&e)
                );
                false
            }
        }
    } else {
        false
    };

    let Host {
        mut peripheral,
        runner,
        ..
    } = stack.build();
    let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "TrouBLE",
        appearance: &appearance::power_device::GENERIC_POWER_DEVICE,
    }))
    .unwrap();

    let _ = join(ble_task(runner), async {
        loop {
            defmt::info!(
                "[adv] advertising loop restarting (bond_stored={})",
                bond_stored
            );
            // Race advertise against restart signal to allow restart even when not connected
            let adv_fut = advertise("Trouble Example", &mut peripheral, &server);
            let restart_fut = ADV_RESTART_SIGNAL.wait();

            match select(adv_fut, restart_fut).await {
                Either::Second(_) => {
                    // Restart signal fired while advertising - restart loop
                    defmt::info!(
                        "[adv] pairing state changed while ADVERTISING (no connection), restarting"
                    );
                    // TODO: do we really need to wait?
                    Timer::after_millis(500).await;
                    continue;
                }
                Either::First(adv_result) => match adv_result {
                    Ok(conn) => {
                        defmt::info!("[adv] connection established, starting tasks");
                        defmt::info!(
                            "[adv] pairing_active={}, bond_stored={}",
                            is_pairing_active(),
                            bond_stored
                        );
                        // Allow bondable in pairing mode (to create new bonds) OR when bond exists (to use existing bond for encryption)
                        // This prevents bonding outside pairing mode but allows reconnection with existing bonds
                        let bondable = is_pairing_active() || bond_stored;
                        defmt::info!("[adv] setting bondable={}", bondable);
                        conn.raw().set_bondable(bondable).unwrap();
                        defmt::info!("[adv] starting connection tasks");
                        // set up tasks when the connection is established to a central, so they don't run when no one is connected.
                        let a = gatt_events_task(&server, &conn, &mut bond_stored);
                        let b = custom_task(&server, &conn, &stack);
                        let c = ADV_RESTART_SIGNAL.wait();
                        // run until any task ends (connection closed or pairing state change)
                        match select3(a, b, c).await {
                            Either3::First(result) => {
                                // gatt_events_task completed - this means disconnect was detected
                                let _ = result;
                                defmt::info!(
                                    "[adv] gatt_events_task completed (disconnect detected), restarting advertising"
                                );
                                ADV_RESTART_SIGNAL.signal(());
                            }
                            Either3::Second(_) => {
                                // custom_task completed (L2CAP tasks exited)
                                // This can happen if L2CAP errors out before gatt_events_task sees the disconnect
                                // We need to wait a bit for the disconnect event to arrive
                                defmt::info!("[adv] custom_task completed, waiting for disconnect event");
                                let disconnect_wait = async {
                                    loop {
                                        match conn.next().await {
                                            GattConnectionEvent::Disconnected { .. } => {
                                                defmt::info!("[adv] disconnect event received after custom_task exit");
                                                break;
                                            }
                                            _ => {}
                                        }
                                    }
                                };
                                // Wait up to 1 second for disconnect event
                                match embassy_time::with_timeout(embassy_time::Duration::from_secs(1), disconnect_wait).await {
                                    Ok(_) => {
                                        defmt::info!("[adv] disconnect confirmed, restarting advertising");
                                    }
                                    Err(_) => {
                                        defmt::warn!("[adv] disconnect event timeout, forcing disconnect");
                                        let _ = conn.raw().disconnect();
                                    }
                                }
                                ADV_RESTART_SIGNAL.signal(());
                            }
                            Either3::Third(_) => {
                                // Restart signal: disconnect and wait for disconnect event
                                defmt::info!(
                                    "[adv] pairing state changed while connected, disconnecting..."
                                );
                                let _ = conn.raw().disconnect();
                                // Wait for disconnect event (with timeout as safety net)
                                let disconnect_fut = async {
                                    loop {
                                        match conn.next().await {
                                            GattConnectionEvent::Disconnected { .. } => break,
                                            _ => {}
                                        }
                                    }
                                };
                                let timeout_fut = Timer::after_millis(1000);
                                match select(disconnect_fut, timeout_fut).await {
                                    Either::First(_) => {
                                        defmt::info!("[adv] disconnected, restarting advertising");
                                    }
                                    Either::Second(_) => {
                                        defmt::warn!("[adv] disconnect timeout, forcing restart");
                                    }
                                }
                                ADV_RESTART_SIGNAL.signal(());
                            }
                        }
                    }
                    Err(e) => {
                        let e = defmt::Debug2Format(&e);
                        defmt::warn!("[adv] error (will retry): {:?}", e);
                        Timer::after_millis(250).await;
                    }
                },
            }
        }
    })
    .await;
}

/// This is a background task that is required to run forever alongside any other BLE tasks.
async fn ble_task<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) {
    loop {
        if let Err(e) = runner.run().await {
            let e = defmt::Debug2Format(&e);
            defmt::error!("[ble_task] error: {:?}", e);
            // Don't panic, just continue and retry
        }
    }
}

/// Stream Events until the connection closes.
///
/// This function will handle the GATT events and process them.
/// This is how we interact with read and write requests.
async fn gatt_events_task(
    _server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    bond_stored: &mut bool,
) -> Result<(), Error> {
    defmt::info!("[gatt_events_task] started");
    let settings = unsafe { settings::get_settings() };
    let reason = loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => {
                defmt::info!(
                    "[peripheral] Disconnected: {:?}",
                    defmt::Debug2Format(&reason)
                );
                defmt::info!("[peripheral] signaling advertising restart after disconnect");
                ADV_RESTART_SIGNAL.signal(());
                break reason;
            }
            GattConnectionEvent::PairingComplete {
                security_level: _,
                bond,
            } => {
                defmt::info!("[peripheral] Pairing complete!");
                if let Some(bond) = bond {
                    settings.ble_bond = common::settings::BleBondSettings::from_bond_info(&bond);
                    settings.ble_bond_write();
                    *bond_stored = true;

                    // Signal pairing completion with the bonded device address
                    let addr = settings.ble_bond.bd_addr;
                    PAIRING_COMPLETE_SIGNAL.signal(addr);

                    // Clear pairing mode
                    PAIRING.store(false, Ordering::Relaxed);
                }
                cancel_pairing_mode();
            }
            GattConnectionEvent::PairingFailed(err) => {
                defmt::warn!(
                    "[peripheral] Pairing failed: {:?}",
                    defmt::Debug2Format(&err)
                );
            }
            GattConnectionEvent::Gatt { event } => {
                defmt::trace!("[peripheral] GATT event");
                let result = if conn.raw().security_level()?.encrypted() {
                    None
                } else {
                    Some(AttErrorCode::INSUFFICIENT_ENCRYPTION)
                };
                let reply_result = if let Some(code) = result {
                    event.reject(code)
                } else {
                    event.accept()
                };
                match reply_result {
                    Ok(reply) => reply.send().await,
                    Err(_) => {}
                }
            }
            _ => {
                // Silently ignore other events (MTU, PhyUpdate, DataLengthUpdate, etc.)
            }
        }
    };
    defmt::info!(
        "[gatt_events_task] completed with reason: {:?}",
        defmt::Debug2Format(&reason)
    );

    // Only clear bond on authentication failures; transient link losses should keep bonds
    // so we can reconnect without re-pairing.
    if let Err(err) = reason.to_result() {
        // Keep bond across transient disconnects; only explicit auth failures elsewhere should clear.
        defmt::warn!(
            "[gatt_events_task] Disconnect reason {:?}, keeping bond",
            defmt::Debug2Format(&err)
        );
    }

    Ok(())
}

/// Create an advertiser to use to connect to a BLE Central, and wait for it to connect.
async fn advertise<'values, 'server, C: Controller>(
    _name: &'values str,
    peripheral: &mut Peripheral<'values, C, DefaultPacketPool>,
    server: &'server Server<'values>,
) -> Result<GattConnection<'values, 'server, DefaultPacketPool>, BleHostError<C::Error>> {
    defmt::info!("[advertise] Starting to advertise");
    let mut advertiser_data = [0; 31];
    let mut scan_rsp_data = [0; 31];

    // Manufacturer Specific Data: Raytac CID (0x068A) + ATS legacy product prefix
    const COMPANY_ID: u16 = 0x068A; // Raytac
    const PROD_PREFIX: &[u8] = &[65, 84, 83, 45, 76, 69, 71];

    // Keep ADV payload small: flags + service UUIDs
    let pairing_on = is_pairing_active();
    // Build ADV based on pairing state
    let adv_len = if pairing_on {
        AdStructure::encode_slice(
            &[
                AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                AdStructure::ManufacturerSpecificData {
                    company_identifier: COMPANY_ID,
                    payload: PROD_PREFIX,
                },
            ],
            &mut advertiser_data[..],
        )?
    } else {
        AdStructure::encode_slice(
            &[AdStructure::Flags(
                LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED,
            )],
            &mut advertiser_data[..],
        )?
    };

    let scan_len = AdStructure::encode_slice(
        &[AdStructure::ShortenedLocalName(&[
            65, 84, 83, 32, 76, 101, 103, 97, 99, 121,
        ])],
        &mut scan_rsp_data[..],
    )?;
    let advertiser = peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected {
                adv_data: &advertiser_data[..adv_len],
                scan_data: &scan_rsp_data[..scan_len],
            },
        )
        .await?;
    defmt::info!("[advertise] Now advertising, waiting for connection...");
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
    defmt::info!("[advertise] Connection accepted!");
    Ok(conn)
}

async fn custom_task<'a, C: Controller, P: PacketPool>(
    _server: &Server<'_>,
    conn: &GattConnection<'_, '_, P>,
    stack: &'a Stack<'a, C, P>,
) {
    defmt::info!("[custom_task] started");
    // Clear any stale disconnect request from previous pairing mode activation
    let _ = take_disconnect_request();

    // PSM for L2CAP channel - must match dongle
    // Two separate L2CAP channels for control and data
    const L2CAP_PSM_CONTROL: u16 = 0x0080;
    const L2CAP_PSM_DATA: u16 = 0x0081;
    const L2CAP_MTU: u16 = 251;
    const PAYLOAD_LEN: u16 = 512;

    // Setup L2CAP channel acceptance with separate configs
    // Control channel: 16 credits for reliable control packet delivery
    let l2cap_control_config = L2capChannelConfig {
        mtu: Some(PAYLOAD_LEN - 6),
        mps: Some(L2CAP_MTU - 4),
        flow_policy: CreditFlowPolicy::Every(5),
        initial_credits: Some(16),
    };

    // Data channel: 8 credits to prevent pool exhaustion and leave room for control packets
    let l2cap_data_config = L2capChannelConfig {
        mtu: Some(PAYLOAD_LEN - 6),
        mps: Some(L2CAP_MTU - 4),
        flow_policy: CreditFlowPolicy::Every(5),
        initial_credits: Some(8),
    };

    defmt::info!(
        "[custom_task] Accepting L2CAP channels on PSM {:04x} (control) and {:04x} (data)",
        L2CAP_PSM_CONTROL,
        L2CAP_PSM_DATA
    );

    // Accept BOTH L2CAP channels - listening on both PSMs simultaneously
    // The central will create both channels sequentially, we accept them as they arrive
    // NOTE: We can't determine which PSM each channel uses, so we rely on creation order
    // Dongle creates: control (0x0080) first, then data (0x0081)

    // Give up after multiple timeouts to avoid getting stuck waiting forever
    const MAX_L2CAP_ACCEPT_ATTEMPTS: u8 = 5;
    let mut accept_attempts = 0u8;

    let l2cap_control_channel = loop {
        match embassy_time::with_timeout(
            embassy_time::Duration::from_secs(10),
            L2capChannel::accept(
                stack,
                conn.raw(),
                &[L2CAP_PSM_CONTROL, L2CAP_PSM_DATA],
                &l2cap_control_config,
            ),
        )
        .await
        {
            Ok(Ok(ch)) => {
                defmt::info!("[custom_task] L2CAP channel 1 accepted (control PSM 0x0080)");
                break ch;
            }
            Ok(Err(e)) => {
                defmt::error!(
                    "[custom_task] Failed to accept L2CAP channel 1: {:?}",
                    defmt::Debug2Format(&e)
                );
                return;
            }
            Err(_) => {
                accept_attempts += 1;
                defmt::warn!(
                    "[custom_task] L2CAP channel 1 accept timeout (attempt {}/{})",
                    accept_attempts,
                    MAX_L2CAP_ACCEPT_ATTEMPTS
                );
                if accept_attempts >= MAX_L2CAP_ACCEPT_ATTEMPTS {
                    defmt::error!(
                        "[custom_task] Failed to accept L2CAP channel 1 after {} attempts, giving up",
                        MAX_L2CAP_ACCEPT_ATTEMPTS
                    );
                    return;
                }
                continue;
            }
        }
    };

    accept_attempts = 0; // Reset for second channel
    let l2cap_data_channel = loop {
        match embassy_time::with_timeout(
            embassy_time::Duration::from_secs(10),
            L2capChannel::accept(
                stack,
                conn.raw(),
                &[L2CAP_PSM_CONTROL, L2CAP_PSM_DATA],
                &l2cap_data_config,
            ),
        )
        .await
        {
            Ok(Ok(ch)) => {
                defmt::info!("[custom_task] L2CAP channel 2 accepted (data PSM 0x0081)");
                break ch;
            }
            Ok(Err(e)) => {
                defmt::error!(
                    "[custom_task] Failed to accept L2CAP channel 2: {:?}",
                    defmt::Debug2Format(&e)
                );
                return;
            }
            Err(_) => {
                accept_attempts += 1;
                defmt::warn!(
                    "[custom_task] L2CAP channel 2 accept timeout (attempt {}/{})",
                    accept_attempts,
                    MAX_L2CAP_ACCEPT_ATTEMPTS
                );
                if accept_attempts >= MAX_L2CAP_ACCEPT_ATTEMPTS {
                    defmt::error!(
                        "[custom_task] Failed to accept L2CAP channel 2 after {} attempts, giving up",
                        MAX_L2CAP_ACCEPT_ATTEMPTS
                    );
                    return;
                }
                continue;
            }
        }
    };

    // Get or initialize L2CAP bridge channels
    // Only enable the L2CAP bridge when BLE mode is active; pairing can still proceed in USB mode.
    while transport_mode::get() != TransportMode::Ble {
        let mode = transport_mode::get();
        defmt::info!(
            "[custom_task] transport mode {:?}, waiting to enable L2CAP bridge",
            mode
        );
        transport_mode::wait_for_change().await;
    }

    let l2cap_channels = crate::ble::l2cap_bridge::get_or_init();

    defmt::info!("[custom_task] L2CAP bridge ready, splitting channels");

    // Split channels into separate readers and writers for independent TX/RX tasks
    let (control_writer, control_reader) = l2cap_control_channel.split();
    let (data_writer, data_reader) = l2cap_data_channel.split();

    defmt::info!(
        "[custom_task] Spawning 4 independent tasks: control_tx, control_rx, data_tx, data_rx"
    );

    // Run 4 independent tasks - one for each direction on each channel
    use embassy_futures::select::{Either4, select4};
    match select4(
        control_tx_task(stack, control_writer, l2cap_channels.control_tx),
        control_rx_task(stack, control_reader, l2cap_channels.control_rx),
        data_tx_task(stack, data_writer, l2cap_channels.data_tx),
        data_rx_task(stack, data_reader, l2cap_channels.data_rx),
    )
    .await
    {
        Either4::First(_) => defmt::info!("[custom_task] control_tx_task exited"),
        Either4::Second(_) => defmt::info!("[custom_task] control_rx_task exited"),
        Either4::Third(_) => defmt::info!("[custom_task] data_tx_task exited"),
        Either4::Fourth(_) => defmt::info!("[custom_task] data_rx_task exited"),
    }

    defmt::info!("[custom_task] exiting - L2CAP channels closed");
}

// Control TX task - sends control responses to dongle
async fn control_tx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a trouble_host::Stack<'a, C, P>,
    mut channel: trouble_host::l2cap::L2capChannelWriter<'a, P>,
    tx_queue: &'static crate::ble::l2cap_bridge::L2capControlTxChannel,
) {
    defmt::info!("[control_tx_task] started");
    let mut tx_buf = [0u8; 1024];
    let mut consecutive_timeouts = 0u8;
    const MAX_CONSECUTIVE_TIMEOUTS: u8 = 3;

    loop {
        let pkt = tx_queue.receive().await;
        defmt::info!(
            "[control_tx_task] TX control: starting send (id={})",
            pkt.id
        );
        let send_start = embassy_time::Instant::now();

        match postcard::to_slice(&pkt, &mut tx_buf) {
            Ok(data) => {
                match embassy_time::with_timeout(
                    embassy_time::Duration::from_secs(5),
                    channel.send(stack, data),
                )
                .await
                {
                    Ok(Ok(())) => {
                        consecutive_timeouts = 0; // Reset counter on success
                        let duration_ms = send_start.elapsed().as_millis();
                        defmt::info!(
                            "[control_tx_task] TX control: send complete (id={}, duration={}ms)",
                            pkt.id,
                            duration_ms
                        );
                    }
                    Ok(Err(e)) => {
                        defmt::error!(
                            "[control_tx_task] L2CAP control send failed: {:?}",
                            defmt::Debug2Format(&e)
                        );
                        ADV_RESTART_SIGNAL.signal(());
                        return;
                    }
                    Err(_) => {
                        consecutive_timeouts += 1;
                        defmt::error!(
                            "[control_tx_task] L2CAP control send timeout ({}/{})",
                            consecutive_timeouts,
                            MAX_CONSECUTIVE_TIMEOUTS
                        );
                        if consecutive_timeouts >= MAX_CONSECUTIVE_TIMEOUTS {
                            defmt::error!(
                                "[control_tx_task] Too many consecutive timeouts, exiting"
                            );
                            ADV_RESTART_SIGNAL.signal(());
                            return;
                        }
                    }
                }
            }
            Err(_) => {
                defmt::error!("[control_tx_task] Failed to serialize control packet");
            }
        }
    }
}

// Data TX task - sends data/stream packets to dongle
async fn data_tx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a trouble_host::Stack<'a, C, P>,
    mut channel: trouble_host::l2cap::L2capChannelWriter<'a, P>,
    tx_queue: &'static crate::ble::l2cap_bridge::L2capDataTxChannel,
) {
    defmt::info!("[data_tx_task] started");
    let mut tx_buf = [0u8; 1024];
    let mut consecutive_timeouts = 0u8;
    const MAX_CONSECUTIVE_TIMEOUTS: u8 = 10; // Higher threshold for data packets (streaming is more tolerant)

    loop {
        let pkt = tx_queue.receive().await;

        match postcard::to_slice(&pkt, &mut tx_buf) {
            Ok(data) => {
                match embassy_time::with_timeout(
                    embassy_time::Duration::from_secs(5),
                    channel.send(stack, data),
                )
                .await
                {
                    Ok(Ok(())) => {
                        consecutive_timeouts = 0; // Reset counter on success
                    }
                    Ok(Err(e)) => {
                        defmt::warn!(
                            "[data_tx_task] L2CAP data send failed (dropping packet): {:?}",
                            defmt::Debug2Format(&e)
                        );
                        // Send errors are fatal - channel is broken
                        ADV_RESTART_SIGNAL.signal(());
                        return;
                    }
                    Err(_) => {
                        consecutive_timeouts += 1;
                        defmt::warn!(
                            "[data_tx_task] L2CAP data send timeout (dropping packet, {}/{})",
                            consecutive_timeouts,
                            MAX_CONSECUTIVE_TIMEOUTS
                        );
                        if consecutive_timeouts >= MAX_CONSECUTIVE_TIMEOUTS {
                            defmt::error!(
                                "[data_tx_task] Too many consecutive timeouts, connection appears dead"
                            );
                            ADV_RESTART_SIGNAL.signal(());
                            return;
                        }
                    }
                }
            }
            Err(_) => {
                defmt::error!("[data_tx_task] Failed to serialize data packet");
            }
        }
    }
}

// Control RX task - receives control requests from dongle
async fn control_rx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a trouble_host::Stack<'a, C, P>,
    mut channel: trouble_host::l2cap::L2capChannelReader<'a, P>,
    rx_queue: &'static crate::ble::l2cap_bridge::L2capControlRxChannel,
) {
    defmt::info!("[control_rx_task] started");
    let mut rx_buf = [0u8; 1024];

    loop {
        // Control channel doesn't need timeout - control packets are infrequent
        // The BLE stack will notify us via error if connection dies
        match channel.receive(stack, &mut rx_buf).await {
            Ok(len) => {
                defmt::info!("[control_rx_task] RX control: {} bytes", len);
                match postcard::from_bytes::<common::protodongers::Packet>(&rx_buf[..len]) {
                    Ok(pkt) => {
                        defmt::info!("[control_rx_task] RX control packet id={}", pkt.id);
                        rx_queue.send(pkt).await;
                    }
                    Err(_) => {
                        defmt::error!("[control_rx_task] Failed to deserialize control packet");
                    }
                }
            }
            Err(e) => {
                defmt::error!(
                    "[control_rx_task] L2CAP control receive error: {:?}",
                    defmt::Debug2Format(&e)
                );
                ADV_RESTART_SIGNAL.signal(());
                return;
            }
        }
    }
}

// Data RX task - receives data/stream packets from dongle
async fn data_rx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a trouble_host::Stack<'a, C, P>,
    mut channel: trouble_host::l2cap::L2capChannelReader<'a, P>,
    rx_queue: &'static crate::ble::l2cap_bridge::L2capDataRxChannel,
) {
    defmt::info!("[data_rx_task] started");
    let mut rx_buf = [0u8; 1024];

    loop {
        // No timeout needed - if connection dies, channel.receive() will error
        // The gatt_events_task handles disconnect detection
        match channel.receive(stack, &mut rx_buf).await {
            Ok(len) => match postcard::from_bytes::<common::protodongers::Packet>(&rx_buf[..len]) {
                Ok(pkt) => {
                    if rx_queue.try_send(pkt).is_err() {
                        defmt::warn!("[data_rx_task] data_rx queue full, dropping data packet");
                    }
                }
                Err(_) => {
                    defmt::error!("[data_rx_task] Failed to deserialize data packet");
                }
            },
            Err(e) => {
                defmt::error!(
                    "[data_rx_task] L2CAP data receive error: {:?}",
                    defmt::Debug2Format(&e)
                );
                ADV_RESTART_SIGNAL.signal(());
                return;
            }
        }
    }
}

use core::sync::atomic::{AtomicBool, Ordering};

static ADV_RESTART_SIGNAL: Signal<ThreadModeRawMutex, ()> = Signal::new();

static PAIRING: AtomicBool = AtomicBool::new(false);
static PAIRING_COMPLETE_SIGNAL: Signal<ThreadModeRawMutex, [u8; 6]> = Signal::new();

static REQ_DISCONNECT: AtomicBool = AtomicBool::new(false);

pub fn request_disconnect() {
    REQ_DISCONNECT.store(true, Ordering::Relaxed);
    ADV_RESTART_SIGNAL.signal(());
}
pub fn take_disconnect_request() -> bool {
    REQ_DISCONNECT.swap(false, Ordering::AcqRel)
}

pub fn enter_pairing_mode() {
    PAIRING.store(true, Ordering::Relaxed);
    ADV_RESTART_SIGNAL.signal(());
}

pub fn cancel_pairing_mode() {
    // TODO differentiate a cancel from completed pairing and a cancel that should restart pairing with changed MSD
    PAIRING.store(false, Ordering::Relaxed);
}

pub fn is_pairing_active() -> bool {
    PAIRING.load(Ordering::Relaxed)
}

pub async fn wait_for_pairing_complete() -> [u8; 6] {
    PAIRING_COMPLETE_SIGNAL.wait().await
}
