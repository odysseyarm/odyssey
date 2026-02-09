use embassy_futures::join::join;
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use protodongers::control::device::TransportMode;
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use trouble_host::prelude::*;

use crate::settings;
use crate::transport_mode;

/// Max number of connections
const CONNECTIONS_MAX: usize = 1;

/// Max number of L2CAP channels (need 2: control + data).
const L2CAP_CHANNELS_MAX: usize = 2;

type Pool = DefaultPacketPool;

// GATT Server definition
#[gatt_server]
struct Server {}

/// Run the BLE stack.
pub async fn run<C>(controller: C, seed: [u8; 32])
where
    C: Controller,
{
    defmt::info!("[adv] advertising task starting");
    let address = Address::random(defmt::unwrap!(crate::utils::device_id()[0..6].try_into()));

    let mut resources: HostResources<Pool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();

    let mut host_seed_rng = ChaCha20Rng::from_seed(seed);
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(address)
        .set_random_generator_seed(&mut host_seed_rng);

    // Restore any persisted bond into the host so reconnections encrypt without re-pairing.
    let mut bond_stored = if settings::is_initialized() {
        let settings = unsafe { settings::get_settings() };
        if let Some(bond_info) = settings.ble_bond.to_bond_info() {
            match stack.add_bond_information(bond_info) {
                Ok(()) => {
                    defmt::info!("[ble] Restored bond from settings into host");
                    true
                }
                Err(e) => {
                    defmt::warn!(
                        "[ble] Failed to restore bond into host: {:?}",
                        defmt::Debug2Format(&e)
                    );
                    false
                }
            }
        } else {
            defmt::info!("[ble] No bond stored in settings");
            false
        }
    } else {
        defmt::warn!("[ble] Settings not initialized, bond storage disabled");
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
                        let bondable = is_pairing_active() || bond_stored;
                        defmt::info!("[adv] setting bondable={}", bondable);
                        conn.raw().set_bondable(bondable).unwrap();
                        defmt::info!("[adv] starting connection tasks");
                        let a = gatt_events_task(&server, &conn, &mut bond_stored);
                        let b = custom_task(&server, &conn, &stack);
                        let c = ADV_RESTART_SIGNAL.wait();
                        match select3(a, b, c).await {
                            Either3::First(result) => {
                                let _ = result;
                                defmt::info!(
                                    "[adv] gatt_events_task completed (disconnect detected), restarting advertising"
                                );
                                ADV_RESTART_SIGNAL.signal(());
                            }
                            Either3::Second(_) => {
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
                                defmt::info!(
                                    "[adv] pairing state changed while connected, disconnecting..."
                                );
                                let _ = conn.raw().disconnect();
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

async fn ble_task<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) {
    loop {
        if let Err(e) = runner.run().await {
            let e = defmt::Debug2Format(&e);
            defmt::error!("[ble_task] error: {:?}", e);
        }
    }
}

async fn gatt_events_task(
    _server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    bond_stored: &mut bool,
) -> Result<(), Error> {
    defmt::info!("[gatt_events_task] started");
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
                    // Store bond in settings if initialized
                    if settings::is_initialized() {
                        let settings = unsafe { settings::get_settings() };
                        settings.ble_bond = settings::BleBondSettings::from_bond_info(&bond);
                        settings.ble_bond_write();
                        defmt::info!("[peripheral] Bond stored to flash");
                    }
                    *bond_stored = true;

                    // Signal pairing completion with the bonded device address
                    let addr = bond.identity.bd_addr.raw();
                    let mut addr_copy = [0u8; 6];
                    addr_copy.copy_from_slice(addr);
                    PAIRING_COMPLETE_SIGNAL.signal(addr_copy);
                    PAIRING.store(false, core::sync::atomic::Ordering::Relaxed);
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
            _ => {}
        }
    };
    defmt::info!(
        "[gatt_events_task] completed with reason: {:?}",
        defmt::Debug2Format(&reason)
    );

    if let Err(err) = reason.to_result() {
        defmt::warn!(
            "[gatt_events_task] Disconnect reason {:?}, keeping bond",
            defmt::Debug2Format(&err)
        );
    }

    Ok(())
}

async fn advertise<'values, 'server, C: Controller>(
    _name: &'values str,
    peripheral: &mut Peripheral<'values, C, DefaultPacketPool>,
    server: &'server Server<'values>,
) -> Result<GattConnection<'values, 'server, DefaultPacketPool>, BleHostError<C::Error>> {
    defmt::info!("[advertise] Starting to advertise");
    let mut advertiser_data = [0; 31];
    let mut scan_rsp_data = [0; 31];

    const COMPANY_ID: u16 = 0x068A; // Raytac
    const PROD_PREFIX: &[u8] = &[65, 84, 83, 45, 76, 69, 71];

    let pairing_on = is_pairing_active();
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
    let _ = take_disconnect_request();

    const L2CAP_PSM_CTRL: u16 = 0x0080;
    const L2CAP_PSM_DATA: u16 = 0x0081;

    const PAYLOAD_LEN_CTRL: u16 = 512;
    const L2CAP_MTU_CTRL: u16 = 251;

    const PAYLOAD_LEN_THROUGHPUT: u16 = 2510;
    const L2CAP_MTU_THROUGHPUT: u16 = 251;

    let l2cap_control_config = L2capChannelConfig {
        mtu: Some(PAYLOAD_LEN_CTRL - 6),
        mps: Some(L2CAP_MTU_CTRL - 4),
        flow_policy: CreditFlowPolicy::Every(4),
        initial_credits: Some(16),
    };

    let l2cap_data_config = L2capChannelConfig {
        mtu: Some(PAYLOAD_LEN_THROUGHPUT - 6),
        mps: Some(L2CAP_MTU_THROUGHPUT - 4),
        flow_policy: CreditFlowPolicy::Every(4),
        initial_credits: Some(16),
    };

    defmt::info!(
        "[custom_task] Accepting L2CAP channels on PSM {:04x} (control) and {:04x} (data)",
        L2CAP_PSM_CTRL,
        L2CAP_PSM_DATA
    );

    const MAX_L2CAP_ACCEPT_ATTEMPTS: u8 = 5;
    let mut accept_attempts = 0u8;

    let l2cap_control_channel = loop {
        match embassy_time::with_timeout(
            embassy_time::Duration::from_secs(10),
            L2capChannel::accept(
                stack,
                conn.raw(),
                &[L2CAP_PSM_CTRL, L2CAP_PSM_DATA],
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

    accept_attempts = 0;
    let l2cap_data_channel = loop {
        match embassy_time::with_timeout(
            embassy_time::Duration::from_secs(10),
            L2capChannel::accept(
                stack,
                conn.raw(),
                &[L2CAP_PSM_CTRL, L2CAP_PSM_DATA],
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

    let (control_writer, control_reader) = l2cap_control_channel.split();
    let (data_writer, data_reader) = l2cap_data_channel.split();

    defmt::info!(
        "[custom_task] Spawning 4 independent tasks: control_tx, control_rx, data_tx, data_rx"
    );

    crate::pmic_leds::set_ble_connected(true);
    crate::power_state::update_led_for_ble_state().await;
    defmt::info!("[custom_task] BLE connected - LED state updated");

    use embassy_futures::select::{select4, Either4};
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

    crate::pmic_leds::set_ble_connected(false);
    crate::power_state::update_led_for_ble_state().await;
    defmt::info!("[custom_task] exiting - L2CAP channels closed, BLE disconnected");
}

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
                        consecutive_timeouts = 0;
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

async fn data_tx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a trouble_host::Stack<'a, C, P>,
    mut channel: trouble_host::l2cap::L2capChannelWriter<'a, P>,
    tx_queue: &'static crate::ble::l2cap_bridge::L2capDataTxChannel,
) {
    defmt::info!("[data_tx_task] started");
    let mut tx_buf = [0u8; 1024];
    let mut consecutive_timeouts = 0u8;
    const MAX_CONSECUTIVE_TIMEOUTS: u8 = 10;

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
                        consecutive_timeouts = 0;
                    }
                    Ok(Err(e)) => {
                        defmt::warn!(
                            "[data_tx_task] L2CAP data send failed (dropping packet): {:?}",
                            defmt::Debug2Format(&e)
                        );
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

async fn control_rx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a trouble_host::Stack<'a, C, P>,
    mut channel: trouble_host::l2cap::L2capChannelReader<'a, P>,
    rx_queue: &'static crate::ble::l2cap_bridge::L2capControlRxChannel,
) {
    defmt::info!("[control_rx_task] started");
    let mut rx_buf = [0u8; 1024];

    loop {
        match channel.receive(stack, &mut rx_buf).await {
            Ok(len) => {
                defmt::info!("[control_rx_task] RX control: {} bytes", len);
                match postcard::from_bytes::<protodongers::Packet>(&rx_buf[..len]) {
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

async fn data_rx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a trouble_host::Stack<'a, C, P>,
    mut channel: trouble_host::l2cap::L2capChannelReader<'a, P>,
    rx_queue: &'static crate::ble::l2cap_bridge::L2capDataRxChannel,
) {
    defmt::info!("[data_rx_task] started");
    let mut rx_buf = [0u8; 1024];

    loop {
        match channel.receive(stack, &mut rx_buf).await {
            Ok(len) => match postcard::from_bytes::<protodongers::Packet>(&rx_buf[..len]) {
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

static ADV_RESTART_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();

static PAIRING: AtomicBool = AtomicBool::new(false);
static PAIRING_COMPLETE_SIGNAL: Signal<CriticalSectionRawMutex, [u8; 6]> = Signal::new();

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
    PAIRING.store(false, Ordering::Relaxed);
}

pub fn is_pairing_active() -> bool {
    PAIRING.load(Ordering::Relaxed)
}

pub async fn wait_for_pairing_complete() -> [u8; 6] {
    PAIRING_COMPLETE_SIGNAL.wait().await
}
