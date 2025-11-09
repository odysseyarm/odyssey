use embassy_futures::join::join;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use trouble_host::prelude::*;

use crate::settings;

/// Max number of connections
const CONNECTIONS_MAX: usize = 1;

/// Max number of L2CAP channels (need 2: control + data).
const L2CAP_CHANNELS_MAX: usize = 2;

// GATT Server definition
#[gatt_server]
struct Server {}

/// Run the BLE stack.
pub async fn run<C>(controller: C)
where
    C: Controller,
{
    let address = Address::random(defmt::unwrap!(crate::utils::device_id()[0..6].try_into()));

    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources).set_random_address(address);

    // Safety: init_settings is called before BLE task is spawned
    let settings = unsafe { settings::get_settings() };

    let mut bond_stored = if let Some(bond_info) = settings.ble_bond.to_bond_info() {
        stack.add_bond_information(bond_info).unwrap();
        true
    } else {
        false
    };

    let Host {
        mut peripheral, runner, ..
    } = stack.build();
    let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "TrouBLE",
        appearance: &appearance::power_device::GENERIC_POWER_DEVICE,
    }))
    .unwrap();

    let _ = join(ble_task(runner), async {
        loop {
            // Race advertise against restart signal to allow restart even when not connected
            let adv_fut = advertise("Trouble Example", &mut peripheral, &server);
            let restart_fut = ADV_RESTART_SIGNAL.wait();

            match select(adv_fut, restart_fut).await {
                Either::Second(_) => {
                    // Restart signal fired while advertising - restart loop
                    defmt::info!("[adv] pairing state changed while ADVERTISING (no connection), restarting");
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
                        // Allow bondable if no bond is stored.
                        let bondable = is_pairing_active() || !bond_stored;
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
                                // gatt_events_task completed
                                let _ = result;
                            }
                            Either3::Second(_) => {
                                // custom_task completed
                            }
                            Either3::Third(_) => {
                                // Restart signal: disconnect and wait for disconnect event
                                defmt::info!("[adv] pairing state changed while connected, disconnecting...");
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
            panic!("[ble_task] error: {:?}", e);
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
                defmt::info!("[peripheral] Disconnected: {:?}", defmt::Debug2Format(&reason));
                break reason;
            }
            GattConnectionEvent::PairingComplete {
                security_level: _,
                bond,
            } => {
                defmt::info!("[peripheral] Pairing complete!");
                if let Some(bond) = bond {
                    settings.ble_bond = crate::settings::BleBondSettings::from_bond_info(&bond);
                    settings.ble_bond_write();
                    *bond_stored = true;
                }
                cancel_pairing_mode();
            }
            GattConnectionEvent::PairingFailed(err) => {
                defmt::warn!("[peripheral] Pairing failed: {:?}", defmt::Debug2Format(&err));
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
            &[AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED)],
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
    let mut l2cap_control_channel = loop {
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
                defmt::debug!("[custom_task] L2CAP accept timeout, retrying...");
                continue;
            }
        }
    };

    let mut l2cap_data_channel = loop {
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
                defmt::debug!("[custom_task] L2CAP accept timeout, retrying...");
                continue;
            }
        }
    };

    // Get or initialize L2CAP bridge channels
    let l2cap_channels = crate::ble::l2cap_bridge::get_or_init();

    defmt::info!("[custom_task] L2CAP bridge ready, entering event loop");

    let mut rx_buf_control = [0u8; 1024];
    let mut rx_buf_data = [0u8; 1024];
    let mut tx_buf = [0u8; 1024];

    loop {
        // Check disconnect request first
        let disconnect_requested = take_disconnect_request();
        defmt::debug!(
            "[custom_task] Loop iteration, disconnect_requested={}",
            disconnect_requested
        );
        if disconnect_requested {
            let _ = conn.raw().disconnect();
            break;
        }

        // Drain all available control TX packets before waiting
        while let Ok(pkt) = l2cap_channels.control_tx.try_receive() {
            match postcard::to_slice(&pkt, &mut tx_buf) {
                Ok(data) => {
                    match embassy_time::with_timeout(
                        embassy_time::Duration::from_secs(5),
                        l2cap_control_channel.send(stack, data),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            defmt::error!("[custom_task] L2CAP control send failed: {:?}", defmt::Debug2Format(&e));
                            return;
                        }
                        Err(_) => {
                            defmt::error!("[custom_task] L2CAP control send timeout");
                            return;
                        }
                    }
                }
                Err(_) => {
                    defmt::error!("[custom_task] Failed to serialize control packet");
                }
            }
        }

        // Drain all available data TX packets before waiting
        while let Ok(pkt) = l2cap_channels.data_tx.try_receive() {
            match postcard::to_slice(&pkt, &mut tx_buf) {
                Ok(data) => {
                    match embassy_time::with_timeout(
                        embassy_time::Duration::from_secs(5),
                        l2cap_data_channel.send(stack, data),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            defmt::warn!(
                                "[custom_task] L2CAP data send failed (dropping packet): {:?}",
                                defmt::Debug2Format(&e)
                            );
                            // Don't return - just drop the packet and continue
                        }
                        Err(_) => {
                            defmt::warn!("[custom_task] L2CAP data send timeout (dropping packet)");
                            // Don't return - just drop the packet and continue
                        }
                    }
                }
                Err(_) => {
                    defmt::error!("[custom_task] Failed to serialize data packet");
                }
            }
        }

        // Wait for next event from either channel
        use embassy_futures::select::{Either3, select3};
        match select3(
            async {
                embassy_futures::select::select(l2cap_channels.control_tx.receive(), l2cap_channels.data_tx.receive())
                    .await
            },
            async {
                embassy_futures::select::select(
                    embassy_time::with_timeout(
                        embassy_time::Duration::from_secs(30),
                        l2cap_control_channel.receive(stack, &mut rx_buf_control),
                    ),
                    embassy_time::with_timeout(
                        embassy_time::Duration::from_secs(30),
                        l2cap_data_channel.receive(stack, &mut rx_buf_data),
                    ),
                )
                .await
            },
            conn.next(),
        )
        .await
        {
            // TX packet ready (either control or data)
            Either3::First(either_pkt) => {
                use embassy_futures::select::Either;
                match either_pkt {
                    Either::First(pkt) => {
                        // Control packet
                        match postcard::to_slice(&pkt, &mut tx_buf) {
                            Ok(data) => {
                                match embassy_time::with_timeout(
                                    embassy_time::Duration::from_secs(5),
                                    l2cap_control_channel.send(stack, data),
                                )
                                .await
                                {
                                    Ok(Ok(())) => {}
                                    Ok(Err(e)) => {
                                        defmt::error!(
                                            "[custom_task] L2CAP control send failed: {:?}",
                                            defmt::Debug2Format(&e)
                                        );
                                        break;
                                    }
                                    Err(_) => {
                                        defmt::error!("[custom_task] L2CAP control send timeout");
                                        break;
                                    }
                                }
                            }
                            Err(_) => {
                                defmt::error!("[custom_task] Failed to serialize control packet");
                            }
                        }
                    }
                    Either::Second(pkt) => {
                        // Data packet - drop on error instead of disconnecting
                        match postcard::to_slice(&pkt, &mut tx_buf) {
                            Ok(data) => {
                                match embassy_time::with_timeout(
                                    embassy_time::Duration::from_secs(5),
                                    l2cap_data_channel.send(stack, data),
                                )
                                .await
                                {
                                    Ok(Ok(())) => {}
                                    Ok(Err(e)) => {
                                        defmt::warn!(
                                            "[custom_task] L2CAP data send failed (dropping packet): {:?}",
                                            defmt::Debug2Format(&e)
                                        );
                                    }
                                    Err(_) => {
                                        defmt::warn!("[custom_task] L2CAP data send timeout (dropping packet)");
                                    }
                                }
                            }
                            Err(_) => {
                                defmt::error!("[custom_task] Failed to serialize data packet");
                            }
                        }
                    }
                }
            }

            // RX packet ready (either control or data)
            Either3::Second(either_rx) => {
                use embassy_futures::select::Either;
                match either_rx {
                    Either::First(timeout_result) => {
                        // Control channel RX - use blocking send to ensure delivery
                        match timeout_result {
                            Ok(Ok(len)) => {
                                defmt::info!("[custom_task] RX control: {} bytes", len);
                                match postcard::from_bytes::<protodongers::Packet>(&rx_buf_control[..len]) {
                                    Ok(pkt) => {
                                        defmt::info!("[custom_task] RX control packet id={}", pkt.id);
                                        // CRITICAL: Control packets must not be dropped
                                        l2cap_channels.rx.send(pkt).await;
                                    }
                                    Err(_) => {
                                        defmt::error!("[custom_task] Failed to deserialize control packet");
                                    }
                                }
                            }
                            Ok(Err(e)) => {
                                defmt::error!(
                                    "[custom_task] L2CAP control receive error: {:?}",
                                    defmt::Debug2Format(&e)
                                );
                                break;
                            }
                            Err(_) => {
                                // Timeout is OK, just means no data
                            }
                        }
                    }
                    Either::Second(timeout_result) => {
                        // Data channel RX
                        match timeout_result {
                            Ok(Ok(len)) => match postcard::from_bytes::<protodongers::Packet>(&rx_buf_data[..len]) {
                                Ok(pkt) => {
                                    if l2cap_channels.rx.try_send(pkt).is_err() {
                                        defmt::warn!("[custom_task] RX channel full, dropping data");
                                    }
                                }
                                Err(_) => {
                                    defmt::error!("[custom_task] Failed to deserialize data packet");
                                }
                            },
                            Ok(Err(e)) => {
                                defmt::error!("[custom_task] L2CAP data receive error: {:?}", defmt::Debug2Format(&e));
                                break;
                            }
                            Err(_) => {
                                // Timeout is OK, just means no data
                            }
                        }
                    }
                }
            }

            // Connection event
            Either3::Third(event) => {
                match event {
                    GattConnectionEvent::Disconnected { reason } => {
                        defmt::info!("[custom_task] Disconnected: {:?}", defmt::Debug2Format(&reason));
                        break;
                    }
                    _ => {
                        // Ignore other connection events
                    }
                }
            }
        }
    }

    defmt::info!("[custom_task] exiting - L2CAP channels closed");

    // L2CAP channels closed (dongle closed them when USB disconnected)
    // Streams will naturally stop as there's no longer a path to send data
    // When L2CAP channels reconnect, streams can be re-enabled via StreamUpdate packets
}
use core::sync::atomic::{AtomicBool, Ordering};

static ADV_RESTART_SIGNAL: Signal<ThreadModeRawMutex, ()> = Signal::new();

static PAIRING: AtomicBool = AtomicBool::new(false);

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
