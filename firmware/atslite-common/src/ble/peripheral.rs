use backon::{ConstantBuilder, EmbassySleeper, Retryable};
use embassy_futures::join::join3;
use embassy_futures::select::{Either3, select3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use trouble_host::prelude::*;

use crate::settings;

const CONNECTIONS_MAX: usize = 1;

/// One L2CAP channel for control packets, one for streaming data.
const L2CAP_CHANNELS_MAX: usize = 2;

type Pool = DefaultPacketPool;
const HOST_BONDS_MAX: usize = 2 * odyssey_protocol::control::MAX_BONDS;
const L2CAP_PSM_CTRL: u16 = 0x0080;
const L2CAP_PSM_DATA: u16 = 0x0081;

/// Largest PDU payload on the data channel: a 251-byte link-layer payload less the four-byte
/// L2CAP header.
pub(crate) const DATA_MPS: u16 = 251 - 4;

#[gatt_server]
struct Server {}

/// Runs the BLE host: loads the stored bonds, advertises, and serves one connection at a time,
/// advertising again whenever the connection ends. Never returns.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn run<C>(
    controller: C,
    seed: [u8; 32],
    device_name: &'static str,
    prod_prefix: &'static [u8],
) where
    C: Controller + bt_hci::controller::ControllerCmdSync<super::netcore::ReadNetCoreRelease>,
{
    defmt::info!("[adv] advertising task starting");
    // A name set earlier through set_device_name_initial() wins over `device_name`.
    CURRENT_NAME.lock(|n| {
        let mut s = n.borrow_mut();
        if s.is_empty() {
            for c in device_name.chars().take(32) {
                let _ = s.push(c);
            }
        }
    });
    let address = Address::random(odyssey_protocol::control::ble_address(defmt::unwrap!(
        crate::utils::device_id()[0..6].try_into()
    )));

    let mut resources: HostResources<Pool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX, 1, HOST_BONDS_MAX> =
        HostResources::new();

    ble_crypto::init(seed);
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(address)
        .register_l2cap_spsm(L2CAP_PSM_CTRL)
        .register_l2cap_spsm(L2CAP_PSM_DATA)
        .build();
    let mut peripheral = stack.peripheral();
    let runner = stack.runner();
    let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: device_name,
        appearance: &appearance::power_device::GENERIC_POWER_DEVICE,
    }))
    .unwrap();

    let _ = join3(ble_task(runner), super::netcore::query(&stack), async {
        loop {
            // Each cycle rebuilds the bond table and the advertising data from current state, so
            // a request raised before this point is already served.
            DISCONNECT_REQUEST.reset();
            ADVERTISING_STALE.reset();
            let bond_stored = sync_bonds(&stack);
            defmt::info!("[adv] advertising (bond_stored={})", bond_stored);

            let current_name = get_device_name();
            let advertised = select3(
                advertise(current_name.as_str(), prod_prefix, &mut peripheral, &server),
                DISCONNECT_REQUEST.wait(),
                ADVERTISING_STALE.wait(),
            )
            .await;
            let conn = match advertised {
                Either3::First(Ok(conn)) => conn,
                Either3::First(Err(e)) => {
                    let e = defmt::Debug2Format(&e);
                    defmt::warn!("[adv] error (will retry): {:?}", e);
                    Timer::after_millis(250).await;
                    continue;
                }
                Either3::Second(()) | Either3::Third(()) => {
                    defmt::info!("[adv] advertising restart requested");
                    Timer::after(ADVERTISING_RESTART_DELAY).await;
                    continue;
                }
            };

            defmt::info!(
                "[adv] connection established, pairing_active={}, bond_stored={}",
                is_pairing_active(),
                bond_stored
            );
            // Bonding is allowed in pairing mode, to create a bond, and when a bond
            // is stored, so the link can encrypt with it.
            let bondable = is_pairing_active() || bond_stored;
            defmt::info!("[adv] setting bondable={}", bondable);
            if let Err(e) = conn.raw().set_bondable(bondable) {
                defmt::warn!(
                    "[adv] Failed to configure connection: {:?}",
                    defmt::Debug2Format(&e)
                );
                disconnect_and_wait(&conn).await;
                continue;
            }
            match select3(
                gatt_events_task(&server, &conn),
                custom_task(&server, &conn, &stack),
                DISCONNECT_REQUEST.wait(),
            )
            .await
            {
                Either3::First(Ok(())) => defmt::info!("[adv] central disconnected"),
                Either3::First(Err(e)) => {
                    defmt::warn!(
                        "[adv] GATT event handling failed: {:?}, dropping the link",
                        defmt::Debug2Format(&e)
                    );
                    disconnect_and_wait(&conn).await;
                }
                Either3::Second(()) => {
                    defmt::info!("[adv] L2CAP session ended, dropping the link");
                    disconnect_and_wait(&conn).await;
                }
                Either3::Third(()) => {
                    defmt::info!("[adv] disconnect requested");
                    disconnect_and_wait(&conn).await;
                }
            }
            // custom_task may have been cancelled by either of the other futures. Cleanup belongs
            // to this connection owner, not to the tail of a cancellable child future.
            crate::ble::l2cap_bridge::channels().drain();
            CONNECTED_PEER.lock(|peer| peer.set(None));
            crate::pmic_leds::set_ble_connected(false);
            crate::power_state::update_led_for_ble_state().await;
        }
    })
    .await;
}

/// Pause before advertising again after a request cut an advertising cycle short.
const ADVERTISING_RESTART_DELAY: Duration = Duration::from_millis(500);

/// How long the controller gets to confirm a local disconnect before advertising resumes.
const DISCONNECT_GRACE: Duration = Duration::from_secs(1);

/// Makes the host's bond table hold exactly the bonds stored in settings, and returns whether
/// there is one. Run before each advertising cycle, so an `AddBond` or `RemoveBond` from
/// device control takes effect on the next connection without a reboot.
fn sync_bonds<C: Controller, P: PacketPool>(stack: &Stack<'_, C, P>) -> bool {
    if !settings::is_initialized() {
        defmt::warn!("[ble] Settings not initialized, bond storage disabled");
        return false;
    }
    let stored: heapless::Vec<BondInformation, HOST_BONDS_MAX> =
        settings::with_settings(|s| s.bond_infos().collect());
    let current: heapless::Vec<BondInformation, HOST_BONDS_MAX> =
        stack.with_bond_information(|bonds| bonds.iter().cloned().collect());
    for bond in current {
        if !stored
            .iter()
            .any(|kept| kept.identity.match_identity(&bond.identity))
        {
            let _ = stack.remove_bond_information(bond.identity);
        }
    }
    let mut loaded = false;
    for bond in stored {
        match stack.add_bond_information(bond) {
            Ok(()) => loaded = true,
            Err(e) => defmt::warn!(
                "[ble] Failed to load bond into host: {:?}",
                defmt::Debug2Format(&e)
            ),
        }
    }
    if !loaded {
        defmt::info!("[ble] No bond stored in settings");
    }
    loaded
}

async fn disconnect_and_wait<P: PacketPool>(conn: &GattConnection<'_, '_, P>) {
    conn.raw().disconnect();
    let disconnected = async {
        loop {
            if let GattConnectionEvent::Disconnected { .. } = conn.next().await {
                break;
            }
        }
    };
    if embassy_time::with_timeout(DISCONNECT_GRACE, disconnected)
        .await
        .is_err()
    {
        defmt::warn!("[adv] no disconnect event within the grace period, advertising anyway");
    }
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
) -> Result<(), Error> {
    defmt::info!("[gatt_events_task] started");
    let reason = loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => {
                defmt::info!(
                    "[peripheral] Disconnected: {:?}",
                    defmt::Debug2Format(&reason)
                );
                break reason;
            }
            GattConnectionEvent::Encrypted {
                bond: Some(bond), ..
            } => {
                settings::with_settings(|s| s.learn_bond_address(&bond));
            }
            GattConnectionEvent::PairingComplete {
                security_level: _,
                bond,
            } => {
                defmt::info!("[peripheral] Pairing complete!");
                if let Some(bond) = bond {
                    let pairing_active = is_pairing_active();
                    let known = settings::is_initialized()
                        && settings::with_settings(|s| s.is_bonded_peer(&bond.identity));
                    if !pairing_active && !known {
                        // A cancelled/expired pairing must not add a bond later.
                        defmt::warn!("[peripheral] Rejecting bond outside the pairing window");
                        request_disconnect();
                        continue;
                    }
                    let stored = !settings::is_initialized()
                        || settings::with_settings(|s| s.bond_store_info(&bond).is_ok());
                    if !stored {
                        defmt::warn!("[peripheral] Rejecting bond: every slot is in use");
                        if pairing_active {
                            PAIRING_COMPLETE_SIGNAL.signal(Err(
                                odyssey_protocol::control::device::PairingError::BondsFull,
                            ));
                        }
                        request_disconnect();
                        continue;
                    }
                    defmt::info!("[peripheral] Bond queued for flash");

                    if pairing_active {
                        let mut addr = [0u8; 6];
                        addr.copy_from_slice(bond.identity.addr.addr.raw());
                        PAIRING_COMPLETE_SIGNAL.signal(Ok(addr));
                        // Keep the successful connection. Only cancellation tears one down.
                        PAIRING.store(false, Ordering::Relaxed);
                        ADVERTISING_STALE.signal(());
                    }
                }
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
    device_name: &str,
    prod_prefix: &[u8],
    peripheral: &mut Peripheral<'values, C, DefaultPacketPool>,
    server: &'server Server<'values>,
) -> Result<GattConnection<'values, 'server, DefaultPacketPool>, BleHostError<C::Error>> {
    defmt::info!("[advertise] Starting to advertise");
    let mut advertiser_data = [0; 31];
    let mut scan_rsp_data = [0; 31];

    const COMPANY_ID: u16 = 0x068A; // Raytac

    let pairing_on = is_pairing_active();
    // A valid 32-byte device name does not fit in a legacy 31-byte advertisement. Include
    // only complete UTF-8 characters and reserve space for flags and pairing manufacturer data.
    let manufacturer_len = if pairing_on { 4 + prod_prefix.len() } else { 0 };
    let adv_name = shortened_name(device_name, 31usize.saturating_sub(5 + manufacturer_len));
    let scan_name = shortened_name(device_name, 29);
    let adv_len = if pairing_on {
        AdStructure::encode_slice(
            &[
                AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                AdStructure::ManufacturerSpecificData {
                    company_identifier: COMPANY_ID,
                    payload: prod_prefix,
                },
                AdStructure::ShortenedLocalName(adv_name.as_bytes()),
            ],
            &mut advertiser_data[..],
        )?
    } else {
        AdStructure::encode_slice(
            &[
                AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                AdStructure::ShortenedLocalName(adv_name.as_bytes()),
            ],
            &mut advertiser_data[..],
        )?
    };

    let scan_len = AdStructure::encode_slice(
        &[AdStructure::ShortenedLocalName(scan_name.as_bytes())],
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
    if pairing_on {
        PAIRING_ADVERTISING.signal(());
    }
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
    defmt::info!("[advertise] Connection accepted!");
    Ok(conn)
}

fn shortened_name(name: &str, max_bytes: usize) -> &str {
    let mut end = name.len().min(max_bytes);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

/// How long one L2CAP accept waits for the central to open the channel.
const L2CAP_ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);

/// Attempts to accept one L2CAP channel. Only timeouts are retried.
const MAX_L2CAP_ACCEPT_ATTEMPTS: usize = 5;

enum AcceptError<E> {
    TimedOut,
    Failed(E),
}

/// Accepts L2CAP channel number `which` (for logging), retrying on timeout with no delay. Logs and
/// returns `None` when the accept fails or every attempt times out.
async fn accept_l2cap<'a, C: Controller, P: PacketPool>(
    stack: &'a Stack<'_, C, P>,
    listener: &L2capChannelListener<'a, C, P>,
    configs: &[(u16, &L2capChannelConfig)],
    which: u8,
) -> Option<(u16, L2capChannel<'a, P>)> {
    let mut attempts = 0usize;
    let accepted = (|| {
        attempts += 1;
        let attempt = attempts;
        async move {
            match embassy_time::with_timeout(L2CAP_ACCEPT_TIMEOUT, async {
                let pending = listener.next().await?;
                let psm = pending.spsm();
                let config = configs
                    .iter()
                    .find(|(candidate, _)| *candidate == psm)
                    .map(|(_, config)| *config)
                    .ok_or(Error::InvalidValue)?;
                let channel = pending.accept(stack, config).await?;
                Ok::<_, BleHostError<C::Error>>((psm, channel))
            })
            .await
            {
                Ok(Ok(ch)) => Ok(ch),
                Ok(Err(e)) => Err(AcceptError::Failed(e)),
                Err(_) => {
                    defmt::warn!(
                        "[custom_task] L2CAP channel {} accept timeout (attempt {}/{})",
                        which,
                        attempt,
                        MAX_L2CAP_ACCEPT_ATTEMPTS
                    );
                    Err(AcceptError::TimedOut)
                }
            }
        }
    })
    .retry(
        ConstantBuilder::default()
            .with_delay(core::time::Duration::ZERO)
            .with_max_times(MAX_L2CAP_ACCEPT_ATTEMPTS - 1),
    )
    .sleep(EmbassySleeper)
    .when(|e| matches!(e, AcceptError::TimedOut))
    .await;
    match accepted {
        Ok(ch) => Some(ch),
        Err(AcceptError::Failed(e)) => {
            defmt::error!(
                "[custom_task] Failed to accept L2CAP channel {}: {:?}",
                which,
                defmt::Debug2Format(&e)
            );
            None
        }
        Err(AcceptError::TimedOut) => {
            defmt::error!(
                "[custom_task] Failed to accept L2CAP channel {} after {} attempts, giving up",
                which,
                MAX_L2CAP_ACCEPT_ATTEMPTS
            );
            None
        }
    }
}

async fn custom_task<'a, C: Controller, P: PacketPool>(
    _server: &Server<'_>,
    conn: &GattConnection<'_, '_, P>,
    stack: &'a Stack<'_, C, P>,
) {
    defmt::info!("[custom_task] started");

    const PAYLOAD_LEN_CTRL: u16 = 512;
    const L2CAP_MTU_CTRL: u16 = 251;

    const PAYLOAD_LEN_THROUGHPUT: u16 = 2510;

    let l2cap_control_config = L2capChannelConfig {
        mtu: Some(PAYLOAD_LEN_CTRL - 6),
        mps: Some(L2CAP_MTU_CTRL - 4),
        flow_policy: CreditFlowPolicy::Every(4),
        initial_credits: Some(16),
    };

    let l2cap_data_config = L2capChannelConfig {
        mtu: Some(PAYLOAD_LEN_THROUGHPUT - 6),
        mps: Some(DATA_MPS),
        flow_policy: CreditFlowPolicy::Every(4),
        initial_credits: Some(16),
    };

    defmt::info!(
        "[custom_task] Accepting L2CAP channels on PSM {:04x} (control) and {:04x} (data)",
        L2CAP_PSM_CTRL,
        L2CAP_PSM_DATA
    );

    let listener = L2capChannel::listen(stack, conn.raw());
    let configs = [
        (L2CAP_PSM_CTRL, &l2cap_control_config),
        (L2CAP_PSM_DATA, &l2cap_data_config),
    ];
    let (mut control, mut data) = (None, None);
    for which in 1..=2 {
        let Some((psm, channel)) = accept_l2cap(stack, &listener, &configs, which).await else {
            return;
        };
        let slot = if psm == L2CAP_PSM_CTRL {
            &mut control
        } else {
            &mut data
        };
        if slot.replace(channel).is_some() {
            defmt::warn!("[custom_task] Duplicate L2CAP PSM");
            return;
        }
    }
    let (Some(l2cap_control_channel), Some(l2cap_data_channel)) = (control, data) else {
        return;
    };

    // L2CAP is independent of GATT: its accept path does not enforce GATT's encryption check.
    // Only a stored peer may issue commands or receive reports, including after pairing.
    let encrypted = conn
        .raw()
        .security_level()
        .is_ok_and(|level| level.encrypted());
    let peer = conn.raw().peer_identity();
    let known_peer =
        settings::is_initialized() && settings::with_settings(|s| s.is_bonded_peer(&peer));
    if !encrypted || !known_peer {
        defmt::warn!("[custom_task] Rejecting unencrypted or unknown L2CAP peer");
        return;
    }
    CONNECTED_PEER.lock(|connected| connected.set(Some(peer)));

    // The pumps run whatever transport carries reports. While USB does, object mode queues only
    // replies to requests that came over BLE, and the link otherwise idles until USB goes away.
    let l2cap_channels = crate::ble::l2cap_bridge::channels();
    l2cap_channels.drain();

    defmt::info!("[custom_task] L2CAP bridge ready, splitting channels");

    let (control_writer, control_reader) = l2cap_control_channel.split();
    let (data_writer, data_reader) = l2cap_data_channel.split();

    defmt::info!(
        "[custom_task] Spawning 4 independent tasks: control_tx, control_rx, data_tx, data_rx"
    );

    crate::pmic_leds::set_ble_connected(true);
    crate::power_state::update_led_for_ble_state().await;
    defmt::info!("[custom_task] BLE connected - LED state updated");

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

    defmt::info!("[custom_task] exiting - L2CAP channels closed, BLE disconnected");
}

async fn control_tx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a trouble_host::Stack<'_, C, P>,
    mut channel: trouble_host::l2cap::L2capChannelWriter<'a, P>,
    tx_queue: &'static crate::ble::l2cap_bridge::L2capControlTxChannel,
) {
    defmt::info!("[control_tx_task] started");
    let mut tx_buf = [0u8; 1024];

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
                        crate::ble::l2cap_bridge::control_sent(tx_queue.is_empty());
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
                        return;
                    }
                    Err(_) => {
                        // send() may already have emitted part of a segmented SDU. Another send
                        // on this channel would be interpreted as its continuation, so reconnect.
                        defmt::error!("[control_tx_task] L2CAP control send timeout, closing link");
                        return;
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
    stack: &'a trouble_host::Stack<'_, C, P>,
    mut channel: trouble_host::l2cap::L2capChannelWriter<'a, P>,
    tx_queue: &'static crate::ble::l2cap_bridge::L2capDataTxChannel,
) {
    defmt::info!("[data_tx_task] started");
    let mut tx_buf = [0u8; 1024];
    let mut held = None;

    loop {
        let first = match held.take() {
            Some(pkt) => pkt,
            None => tx_queue.receive().await,
        };
        let queued = || tx_queue.try_receive().ok();
        let used = match crate::packing::pack_queued(first, queued, &mut tx_buf) {
            Ok((used, unsent)) => {
                held = unsent;
                used
            }
            Err(e) => {
                defmt::error!("[data_tx_task] Failed to serialize data packet: {:?}", e);
                continue;
            }
        };

        match embassy_time::with_timeout(
            embassy_time::Duration::from_secs(5),
            channel.send(stack, &tx_buf[..used]),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                defmt::warn!(
                    "[data_tx_task] L2CAP data send failed (dropping packet): {:?}",
                    defmt::Debug2Format(&e)
                );
                return;
            }
            Err(_) => {
                // Dropping a timed-out send may leave a partial SDU on the peer.
                defmt::warn!("[data_tx_task] L2CAP data send timeout, closing link");
                return;
            }
        }
    }
}

async fn control_rx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a trouble_host::Stack<'_, C, P>,
    mut channel: trouble_host::l2cap::L2capChannelReader<'a, P>,
    rx_queue: &'static crate::ble::l2cap_bridge::L2capControlRxChannel,
) {
    defmt::info!("[control_rx_task] started");
    let mut rx_buf = [0u8; 1024];

    loop {
        match channel.receive(stack, &mut rx_buf).await {
            Ok(len) => {
                defmt::info!("[control_rx_task] RX control: {} bytes", len);
                match postcard::from_bytes::<odyssey_protocol::Packet>(&rx_buf[..len]) {
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
                return;
            }
        }
    }
}

async fn data_rx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a trouble_host::Stack<'_, C, P>,
    mut channel: trouble_host::l2cap::L2capChannelReader<'a, P>,
    rx_queue: &'static crate::ble::l2cap_bridge::L2capDataRxChannel,
) {
    defmt::info!("[data_rx_task] started");
    let mut rx_buf = [0u8; 1024];

    loop {
        match channel.receive(stack, &mut rx_buf).await {
            Ok(len) => match postcard::from_bytes::<odyssey_protocol::Packet>(&rx_buf[..len]) {
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
                return;
            }
        }
    }
}

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as BmRaw;

/// Drop the current connection, or restart advertising when there is none. `run` reloads the
/// bond and rebuilds the advertising data before it advertises again.
static DISCONNECT_REQUEST: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The advertising data no longer matches the pairing state. Only an advertising cycle in
/// progress restarts on it. A connection stays up, and the next cycle builds fresh data anyway.
static ADVERTISING_STALE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

static PAIRING: AtomicBool = AtomicBool::new(false);
static PAIRING_COMPLETE_SIGNAL: Signal<
    CriticalSectionRawMutex,
    Result<[u8; 6], odyssey_protocol::control::device::PairingError>,
> = Signal::new();
/// Raised once an advertising cycle with the pairing data is running, so device control can tell
/// the host to pair only when the unit is actually connectable as a new peer.
static PAIRING_ADVERTISING: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Identity of the bonded host whose L2CAP session is open.
static CONNECTED_PEER: BlockingMutex<BmRaw, core::cell::Cell<Option<Identity>>> =
    BlockingMutex::new(core::cell::Cell::new(None));

static CURRENT_NAME: BlockingMutex<BmRaw, RefCell<heapless::String<32>>> =
    BlockingMutex::new(RefCell::new(heapless::String::new()));

/// Sets the advertised name without restarting advertising. Before `run()` this replaces its
/// `device_name`. Afterwards the name takes effect on the next advertising cycle.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn set_device_name_initial(name: &str) {
    CURRENT_NAME.lock(|n| {
        let mut s = n.borrow_mut();
        s.clear();
        for c in name.chars().take(32) {
            let _ = s.push(c);
        }
    });
}

/// Since: Odyssey Desktop 0.7.0.
pub fn get_device_name() -> heapless::String<32> {
    CURRENT_NAME.lock(|n| n.borrow().clone())
}

/// Since: Odyssey Desktop 0.7.0.
pub fn request_disconnect() {
    DISCONNECT_REQUEST.signal(());
}

/// Advertises the pairing data and makes the next connection bondable. Drops the current
/// connection so the host pairs from scratch, and forgets a completion left over from an
/// earlier pairing window.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn enter_pairing_mode() {
    PAIRING_COMPLETE_SIGNAL.reset();
    PAIRING_ADVERTISING.reset();
    PAIRING.store(true, Ordering::Relaxed);
    DISCONNECT_REQUEST.signal(());
}

/// Resolves once advertising with the pairing data has started after [`enter_pairing_mode`].
///
/// Since: Unreleased.
pub async fn wait_for_pairing_advertising() {
    PAIRING_ADVERTISING.wait().await;
}

/// Leaves pairing mode and aborts an in-flight pairing connection. Successful pairing clears
/// `PAIRING` before notifying device control, so acknowledging success keeps the link up.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn cancel_pairing_mode() {
    if PAIRING.swap(false, Ordering::Relaxed) {
        DISCONNECT_REQUEST.signal(());
    }
    ADVERTISING_STALE.signal(());
}

/// Reloads stored bonds into the BLE host by restarting an advertising cycle in progress. An
/// open link from another host stays up; the bonds load before the next cycle starts.
///
/// Since: Unreleased.
pub fn reload_bonds() {
    ADVERTISING_STALE.signal(());
}

/// Since: Odyssey Desktop 0.7.0.
pub fn is_pairing_active() -> bool {
    PAIRING.load(Ordering::Relaxed)
}

/// Since: Odyssey Desktop 0.7.0.
pub async fn wait_for_pairing_complete() -> odyssey_protocol::control::device::PairingOutcome {
    match PAIRING_COMPLETE_SIGNAL.wait().await {
        Ok(addr) => odyssey_protocol::control::device::PairingOutcome::Ok(
            odyssey_protocol::control::BondedDevice {
                uuid: addr,
                name: get_device_name().into(),
            },
        ),
        Err(error) => odyssey_protocol::control::device::PairingOutcome::Err(error),
    }
}

/// The stored bonds, marking the host whose link is open.
///
/// Since: Unreleased.
pub fn bond_slots() -> odyssey_protocol::List<
    odyssey_protocol::control::BondSlot,
    { odyssey_protocol::control::MAX_BONDS },
> {
    let connected = CONNECTED_PEER.lock(core::cell::Cell::get);
    let mut slots = odyssey_protocol::List::default();
    if settings::is_initialized() {
        settings::with_settings(|s| {
            for bond in s.bond_infos() {
                let mut bd_addr = [0u8; 6];
                bd_addr.copy_from_slice(bond.identity.addr.addr.raw());
                if slots
                    .0
                    .iter()
                    .any(|slot: &odyssey_protocol::control::BondSlot| slot.bd_addr == bd_addr)
                {
                    continue;
                }
                let _ = slots.0.push(odyssey_protocol::control::BondSlot {
                    bd_addr,
                    connected: connected.is_some_and(|peer| bond.identity.match_identity(&peer)),
                });
            }
        });
    }
    slots
}

/// Whether an encrypted, bonded L2CAP session is up.
///
/// Since: Unreleased.
pub fn is_connected() -> bool {
    CONNECTED_PEER.lock(|peer| peer.get().is_some())
}

/// Whether the host with identity address `bd_addr` holds the BLE link.
///
/// Since: Unreleased.
pub fn is_connected_peer(bd_addr: [u8; 6]) -> bool {
    CONNECTED_PEER.lock(|peer| {
        peer.get()
            .is_some_and(|identity| identity.addr.addr.raw() == bd_addr.as_slice())
    })
}
