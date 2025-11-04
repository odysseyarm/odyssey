use embassy_futures::join::join;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use trouble_host::prelude::*;

use crate::settings;

/// Max number of connections
const CONNECTIONS_MAX: usize = 1;

/// Max number of L2CAP channels.
const L2CAP_CHANNELS_MAX: usize = 1;

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
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
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
    const L2CAP_PSM: u16 = 0x0080;
    const L2CAP_MTU: u16 = 251;
    const PAYLOAD_LEN: u16 = 1024;

    // Setup L2CAP channel acceptance
    let l2cap_config = L2capChannelConfig {
        mtu: Some(PAYLOAD_LEN - 6),
        mps: Some(L2CAP_MTU - 4),
        flow_policy: CreditFlowPolicy::Every(10), // Replenish credits more frequently
        initial_credits: Some(500),               // Larger initial credit pool for high throughput
    };

    defmt::info!("[custom_task] Accepting L2CAP channel on PSM {:04x}", L2CAP_PSM);

    // Use with_timeout to detect connection drops during accept
    // See: https://github.com/embassy-rs/trouble/issues/355
    let mut l2cap_channel = loop {
        match embassy_time::with_timeout(
            embassy_time::Duration::from_secs(1),
            L2capChannel::accept(stack, conn.raw(), &[L2CAP_PSM], &l2cap_config),
        )
        .await
        {
            Ok(Ok(ch)) => {
                defmt::info!("[custom_task] L2CAP channel accepted");
                break ch;
            }
            Ok(Err(e)) => {
                defmt::error!(
                    "[custom_task] Failed to accept L2CAP channel: {:?}",
                    defmt::Debug2Format(&e)
                );
                return;
            }
            Err(_) => {
                // Timeout - check if still connected
                defmt::debug!("[custom_task] L2CAP accept timeout, retrying...");
                // Connection event check will happen in select3 above if disconnected
                continue;
            }
        }
    };

    // Get or initialize L2CAP bridge channels
    let (l2cap_rx, l2cap_tx) = crate::ble::l2cap_bridge::get_or_init();

    defmt::info!("[custom_task] L2CAP bridge ready, entering event loop");

    let mut rx_buf = [0u8; 1024];
    let mut tx_buf = [0u8; 1024];
    let mut rssi_timer = embassy_time::Timer::after_secs(1);

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

        // Drain all available TX packets before waiting
        while let Ok(pkt) = l2cap_tx.try_receive() {
            match postcard::to_slice(&pkt, &mut tx_buf) {
                Ok(data) => {
                    match embassy_time::with_timeout(
                        embassy_time::Duration::from_secs(5),
                        l2cap_channel.send(stack, data),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            defmt::error!("[custom_task] L2CAP send failed: {:?}", defmt::Debug2Format(&e));
                            return;
                        }
                        Err(_) => {
                            defmt::error!("[custom_task] L2CAP send timeout");
                            return;
                        }
                    }
                }
                Err(_) => {
                    defmt::error!("[custom_task] Failed to serialize packet");
                }
            }
        }

        // Wait for next event, including connection events to detect disconnection
        use embassy_futures::select::{Either4, select4};
        match select4(
            l2cap_tx.receive(),
            embassy_time::with_timeout(
                embassy_time::Duration::from_secs(30),
                l2cap_channel.receive(stack, &mut rx_buf),
            ),
            &mut rssi_timer,
            conn.next(),
        )
        .await
        {
            Either4::First(pkt) => match postcard::to_slice(&pkt, &mut tx_buf) {
                Ok(data) => {
                    match embassy_time::with_timeout(
                        embassy_time::Duration::from_secs(5),
                        l2cap_channel.send(stack, data),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            defmt::error!("[custom_task] L2CAP send failed: {:?}", defmt::Debug2Format(&e));
                            break;
                        }
                        Err(_) => {
                            defmt::error!("[custom_task] L2CAP send timeout");
                            break;
                        }
                    }
                }
                Err(_) => {
                    defmt::error!("[custom_task] Failed to serialize packet");
                }
            },

            Either4::Second(timeout_result) => {
                match timeout_result {
                    Ok(Ok(len)) => {
                        match postcard::from_bytes::<protodongers::Packet>(&rx_buf[..len]) {
                            Ok(pkt) => {
                                // Use try_send to avoid blocking main loop
                                if l2cap_rx.try_send(pkt).is_err() {
                                    defmt::warn!("[custom_task] RX channel full, dropping");
                                }
                            }
                            Err(_) => {
                                defmt::error!("[custom_task] Failed to deserialize packet");
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        defmt::error!("[custom_task] L2CAP receive error: {:?}", defmt::Debug2Format(&e));
                        break;
                    }
                    Err(_) => {
                        defmt::error!("[custom_task] L2CAP receive timeout (30s) - connection may be stalled");
                        break;
                    }
                }
            }

            Either4::Third(_) => {
                let _ = conn.raw().rssi(stack).await;
                rssi_timer = embassy_time::Timer::after_secs(1);
            }

            Either4::Fourth(event) => {
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

    defmt::info!("[custom_task] exiting");
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
