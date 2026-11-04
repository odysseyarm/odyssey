use core::cell::RefCell;
use core::sync::atomic::Ordering;

use backon::{ConstantBuilder, EmbassySleeper, RetryableWithContext};
use defmt::{error, info, warn};
use embassy_futures::select::select;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant};
use odyssey_protocol::mux::DevicePacket;
use trouble_host::Stack;
use trouble_host::prelude::{
    AddrKind, BdAddr, Central, ConnectConfig, ConnectParams, Connection, CreditFlowPolicy,
    L2capChannel, L2capChannelConfig, PhySet, ScanConfig,
};

use super::DevicePacketChannel;
use super::central::{
    ActiveConnections, DISCONNECT_COUNT, DROPPED_DATA_TO_HOST, DeviceQueue, DeviceQueues,
    L2CAP_PSM_CONTROL, L2CAP_PSM_DATA,
};
use super::task_slots::{self, TaskSlot};

/// Wait between the end of one connection attempt or session and the next
/// connect, whatever ended it. A device that keeps failing pairing or
/// encryption is retried at this pace rather than in a tight loop.
const RETRY_BACKOFF: core::time::Duration = core::time::Duration::from_secs(5);

/// Owns one device for its whole lifetime: connect, pair or encrypt, open the
/// L2CAP channels, relay packets, and reconnect [`RETRY_BACKOFF`] after any
/// failure or disconnect. Builds its own Central from the shared stack.
///
/// Runs until [`task_slots::stop_requested`] fires for `slot`. A stop drops
/// whatever step is in progress, disconnects, removes the device from
/// `active_connections` and `device_queues`, and only then releases `slot`.
#[embassy_executor::task(pool_size = odyssey_protocol::mux::MAX_DEVICES)]
pub async fn device_connection_task(
    stack: &'static Stack<'static, crate::Controller, crate::Pool>,
    device_packets: &'static DevicePacketChannel,
    active_connections: &'static ActiveConnections,
    device_queues: &'static DeviceQueues,
    settings: &'static crate::storage::Settings,
    slot: TaskSlot,
    target: BdAddr,
) {
    info!("Device task started for {:02x}", target);

    let host = stack.build();
    let central = host.central;
    let device = Device {
        stack,
        device_packets,
        active_connections,
        device_queues,
        settings,
        target,
    };

    let conn = RefCell::new(None);
    let supervise = async {
        // An attempt always ends in `Err`, so backon waits `RETRY_BACKOFF` and
        // reconnects until the stop signal drops this future.
        let conn = &conn;
        let attempt = |mut central| async move {
            device
                .connect_and_relay(&mut central, &mut *conn.borrow_mut())
                .await;
            device.release(conn).await;
            (central, Err::<(), Reconnect>(Reconnect))
        };
        let _ = attempt
            .retry(
                ConstantBuilder::new()
                    .with_delay(RETRY_BACKOFF)
                    .without_max_times(),
            )
            .context(central)
            .sleep(EmbassySleeper)
            .await;
    };
    select(task_slots::stop_requested(slot), supervise).await;
    device.release(&conn).await;

    info!("Device task for {:02x} stopped", target);
    task_slots::release(slot);
    // The address may have been re-added while this task was stopping, and the
    // manager cannot spawn its replacement until the slot is free.
    crate::ble::central::RESTART_CENTRAL.signal(());
}

/// The retry error of [`device_connection_task`]: every attempt ends and is
/// followed by a reconnect.
struct Reconnect;

#[derive(Clone, Copy)]
struct Device {
    stack: &'static Stack<'static, crate::Controller, crate::Pool>,
    device_packets: &'static DevicePacketChannel,
    active_connections: &'static ActiveConnections,
    device_queues: &'static DeviceQueues,
    settings: &'static crate::storage::Settings,
    target: BdAddr,
}

impl Device {
    /// Disconnects the stored connection and removes the device from the
    /// active-connection and device-queue registries. Safe to repeat.
    async fn release(&self, conn: &RefCell<Option<Connection<'static, crate::Pool>>>) {
        if let Some(conn) = conn.borrow_mut().take() {
            conn.disconnect();
        }
        self.active_connections
            .remove(&self.target.into_inner())
            .await;
        self.device_queues
            .unregister(&self.target.into_inner())
            .await;
    }

    /// One connection attempt and, if it gets that far, the session that
    /// follows. Returns on the first failure or when the session ends.
    ///
    /// The connection is stored in `conn` so the caller can disconnect it even
    /// when this future is dropped part way. The caller also undoes the device
    /// queue and active-connection registration.
    async fn connect_and_relay(
        self,
        central: &mut Central<'static, crate::Controller, crate::Pool>,
        conn: &mut Option<Connection<'static, crate::Pool>>,
    ) {
        let Device {
            stack,
            active_connections,
            device_queues,
            settings,
            target,
            ..
        } = self;

        let config = ConnectConfig {
            connect_params: ConnectParams {
                min_connection_interval: Duration::from_micros(7500),
                max_connection_interval: Duration::from_micros(7500),
                max_latency: 0,
                min_event_length: Duration::from_micros(0),
                max_event_length: Duration::from_micros(0),
                supervision_timeout: Duration::from_secs(2),
            },
            scan_config: ScanConfig {
                filter_accept_list: &[(AddrKind::RANDOM, &target)],
                phys: PhySet::M1,
                ..Default::default()
            },
        };

        info!("Connecting to {:02x}...", target);
        let connect_lease = crate::ble::central::begin_connect().await;
        let connect_result =
            embassy_time::with_timeout(Duration::from_secs(10), central.connect(&config)).await;
        drop(connect_lease);

        let conn = match connect_result {
            Ok(Ok(new_conn)) => {
                info!("Connected to {:02x}", target);
                conn.insert(new_conn)
            }
            Ok(Err(e)) => {
                error!(
                    "Connection failed for {:02x}: {:?}",
                    target,
                    defmt::Debug2Format(&e)
                );
                return;
            }
            Err(_) => {
                error!("Connect timeout for {:02x}", target);
                return;
            }
        };

        let has_bond = settings.get_bond(target).await.is_some();
        let _ = conn.set_bondable(true);

        if !has_bond {
            info!("No bond found, initiating pairing for {:02x}", target);
            if let Err(e) = conn.request_security() {
                error!("Failed to request security: {:?}", defmt::Debug2Format(&e));
                return;
            }

            let pairing_result = embassy_time::with_timeout(Duration::from_secs(10), async {
                loop {
                    match conn.next().await {
                        trouble_host::prelude::ConnectionEvent::PairingComplete {
                            security_level,
                            bond,
                        } => {
                            info!("Pairing complete: {:?}", security_level);
                            if let Some(bi) = bond {
                                let bd = crate::ble::security::bonddata_from_info(&bi);
                                info!("Storing bond for {:02x}", bd.bd_addr);
                                crate::ble::security::BOND_COMMANDS
                                    .send(crate::ble::security::BondCommand::Store(bd))
                                    .await;
                            }
                            return Ok(());
                        }
                        trouble_host::prelude::ConnectionEvent::PairingFailed(err) => {
                            error!("Pairing failed: {:?}", err);
                            return Err(());
                        }
                        trouble_host::prelude::ConnectionEvent::Disconnected { reason } => {
                            error!("Disconnected during pairing: {:?}", reason);
                            return Err(());
                        }
                        _ => {}
                    }
                }
            })
            .await;

            match pairing_result {
                Ok(Ok(())) => info!("Pairing successful"),
                Ok(Err(())) | Err(_) => {
                    error!("Pairing failed or timed out");
                    return;
                }
            }
        } else {
            info!("Using existing bond for {:02x}", target);
            if let Err(e) = conn.request_security() {
                error!(
                    "Failed to request security for bonded device: {:?}",
                    defmt::Debug2Format(&e)
                );
                return;
            }

            let security_result = embassy_time::with_timeout(Duration::from_secs(10), async {
                loop {
                    match conn.next().await {
                        trouble_host::prelude::ConnectionEvent::PairingComplete {
                            security_level,
                            bond,
                        } => {
                            info!("Encryption established: {:?}", security_level);
                            // A bonded peer may pair again and replace its LTK.
                            // Persist that replacement just like an initial bond.
                            if let Some(bi) = bond {
                                let bd = crate::ble::security::bonddata_from_info(&bi);
                                crate::ble::security::BOND_COMMANDS
                                    .send(crate::ble::security::BondCommand::Store(bd))
                                    .await;
                            }
                            return Ok(());
                        }
                        trouble_host::prelude::ConnectionEvent::PairingFailed(err) => {
                            error!("Encryption failed: {:?}", err);
                            return Err(());
                        }
                        trouble_host::prelude::ConnectionEvent::Disconnected { reason } => {
                            error!("Disconnected during encryption: {:?}", reason);
                            return Err(());
                        }
                        _ => {}
                    }
                }
            })
            .await;

            match security_result {
                Ok(Ok(())) => info!("Encryption successful"),
                Ok(Err(())) | Err(_) => {
                    error!("Encryption failed or timed out");
                    return;
                }
            }
        }

        match conn
            .set_phy(stack, trouble_host::prelude::PhyKind::Le2M)
            .await
        {
            Ok(()) => info!("Requested 2M PHY"),
            Err(e) => match e {
                trouble_host::BleHostError::BleHost(kind) => {
                    error!("2M PHY not supported or change failed: {:?}", kind)
                }
                trouble_host::BleHostError::Controller(_) => {
                    error!("2M PHY change controller error")
                }
            },
        }

        const PAYLOAD_LEN: u16 = 2510;
        const L2CAP_MTU: u16 = 251;

        let l2cap_control_config = L2capChannelConfig {
            mtu: Some(PAYLOAD_LEN - 6),
            mps: Some(L2CAP_MTU - 4),
            flow_policy: CreditFlowPolicy::Every(4),
            initial_credits: Some(16),
        };

        let l2cap_data_config = L2capChannelConfig {
            mtu: Some(PAYLOAD_LEN - 6),
            mps: Some(L2CAP_MTU - 4),
            flow_policy: CreditFlowPolicy::Every(4),
            initial_credits: Some(16),
        };

        info!(
            "Setting up L2CAP control channel with PSM {:04x}",
            L2CAP_PSM_CONTROL
        );
        let l2cap_control_channel = match embassy_time::with_timeout(
            Duration::from_secs(10),
            L2capChannel::create(stack, conn, L2CAP_PSM_CONTROL, &l2cap_control_config),
        )
        .await
        {
            Ok(Ok(ch)) => ch,
            Ok(Err(e)) => {
                error!(
                    "Failed to create L2CAP control channel: {:?}",
                    defmt::Debug2Format(&e)
                );
                return;
            }
            Err(_) => {
                error!("L2CAP control channel creation timeout");
                return;
            }
        };

        info!(
            "Setting up L2CAP data channel with PSM {:04x}",
            L2CAP_PSM_DATA
        );
        let l2cap_data_channel = match embassy_time::with_timeout(
            Duration::from_secs(10),
            L2capChannel::create(stack, conn, L2CAP_PSM_DATA, &l2cap_data_config),
        )
        .await
        {
            Ok(Ok(ch)) => ch,
            Ok(Err(e)) => {
                error!(
                    "Failed to create L2CAP data channel: {:?}",
                    defmt::Debug2Format(&e)
                );
                return;
            }
            Err(_) => {
                error!("L2CAP data channel creation timeout");
                return;
            }
        };

        let Some(device_queue) = device_queues.register(&target.into_inner()).await else {
            error!("Failed to register device queue for {:02x}", target);
            return;
        };

        if !active_connections.add(&target.into_inner()).await {
            error!("Failed to add device to active connections");
            return;
        }

        info!(
            "Device {:02x} fully connected, starting communication",
            target
        );

        run_connection(
            self,
            conn,
            l2cap_control_channel,
            l2cap_data_channel,
            device_queue,
        )
        .await;

        info!("Device {:02x} disconnected, cleaning up", target);
    }
}

/// Relays packets in both directions until the link drops or either L2CAP
/// channel fails.
async fn run_connection(
    device: Device,
    conn: &Connection<'static, crate::Pool>,
    l2cap_control_channel: L2capChannel<'static, crate::Pool>,
    l2cap_data_channel: L2capChannel<'static, crate::Pool>,
    device_queue: &'static DeviceQueue,
) {
    let Device {
        stack,
        device_packets,
        settings,
        target,
        ..
    } = device;
    let (mut control_writer, mut control_reader) = l2cap_control_channel.split();
    let (mut data_writer, mut data_reader) = l2cap_data_channel.split();

    let shutdown = Signal::<ThreadModeRawMutex, ()>::new();

    let tx_fut = async {
        let mut tx_buf = [0u8; 1024];
        loop {
            let device_pkt = match embassy_futures::select::select(
                device_queue.receive(),
                shutdown.wait(),
            )
            .await
            {
                embassy_futures::select::Either::First(pkt) => pkt,
                embassy_futures::select::Either::Second(()) => return,
            };
            let is_control = device_pkt.pkt.is_control();

            // Mirror SetDeviceName into the dongle's stored name and name cache.
            if let odyssey_protocol::PacketData::SetDeviceName(name) = &device_pkt.pkt.data {
                settings.set_device_name(target.into_inner(), &name.0).await;
                crate::ble::central::update_device_name(target.into_inner(), name.0.clone());
            }

            if let Ok(data) = postcard::to_slice(&device_pkt.pkt, &mut tx_buf) {
                let ch = if is_control { "ctrl" } else { "data" };
                if matches!(
                    device_pkt.pkt.data,
                    odyssey_protocol::PacketData::WriteConfig(_)
                        | odyssey_protocol::PacketData::FlashSettings()
                ) {
                    info!(
                        "BLE TX [{}]: id={} {:?}",
                        ch,
                        device_pkt.pkt.id,
                        defmt::Debug2Format(&device_pkt.pkt.data)
                    );
                }
                let writer = if is_control {
                    &mut control_writer
                } else {
                    &mut data_writer
                };
                let start = Instant::now();
                match embassy_time::with_timeout(Duration::from_secs(10), writer.send(stack, data))
                    .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        error!("L2CAP send failed [{}]: {:?}", ch, defmt::Debug2Format(&e));
                        shutdown.signal(());
                        return;
                    }
                    Err(_) => {
                        warn!(
                            "L2CAP send timeout [{}]; packet dropped id={}",
                            ch, device_pkt.pkt.id
                        );
                    }
                }
                let elapsed = Instant::now() - start;
                if elapsed >= Duration::from_millis(100) {
                    warn!("L2CAP send [{}] took {} ms", ch, elapsed.as_millis());
                }
            } else {
                error!("Failed to serialize packet");
            }
        }
    };

    let ctrl_rx_fut = async {
        let mut rx_buf = [0u8; 1024];
        loop {
            match control_reader.receive(stack, &mut rx_buf).await {
                Ok(len) => match postcard::from_bytes::<odyssey_protocol::Packet>(&rx_buf[..len]) {
                    Ok(pkt) => {
                        let device_packet = DevicePacket {
                            dev: target.into_inner(),
                            pkt,
                        };
                        device_packets.send(device_packet).await;
                    }
                    Err(_) => error!("Failed to deserialize L2CAP control packet"),
                },
                Err(e) => {
                    error!("L2CAP control receive error: {:?}", defmt::Debug2Format(&e));
                    shutdown.signal(());
                    return;
                }
            }
        }
    };

    let data_rx_fut = async {
        let mut rx_buf = [0u8; 1024];
        loop {
            match data_reader.receive(stack, &mut rx_buf).await {
                Ok(len) => match postcard::from_bytes::<odyssey_protocol::Packet>(&rx_buf[..len]) {
                    Ok(pkt) => {
                        let device_packet = DevicePacket {
                            dev: target.into_inner(),
                            pkt,
                        };
                        if device_packets.try_send(device_packet).is_err() {
                            let c = DROPPED_DATA_TO_HOST.fetch_add(1, Ordering::AcqRel) + 1;
                            if c % 64 == 0 {
                                warn!("Dropped {} data packets to host (channel full)", c);
                            }
                        }
                    }
                    Err(_) => error!("Failed to deserialize L2CAP data packet"),
                },
                Err(e) => {
                    error!("L2CAP data receive error: {:?}", defmt::Debug2Format(&e));
                    shutdown.signal(());
                    return;
                }
            }
        }
    };

    let conn_fut = async {
        loop {
            match conn.next().await {
                trouble_host::prelude::ConnectionEvent::Disconnected { reason } => {
                    let c = DISCONNECT_COUNT.fetch_add(1, Ordering::AcqRel) + 1;
                    info!("Disconnected: {:?} (total={})", reason, c);
                    shutdown.signal(());
                    return reason;
                }
                trouble_host::prelude::ConnectionEvent::PhyUpdated { tx_phy, rx_phy } => {
                    info!("PHY updated tx={:?} rx={:?}", tx_phy, rx_phy);
                }
                trouble_host::prelude::ConnectionEvent::PairingComplete {
                    security_level,
                    bond,
                } => {
                    info!("Pairing complete: {:?}", security_level);
                    if let Some(bi) = bond {
                        let bd = crate::ble::security::bonddata_from_info(&bi);
                        info!("Storing bond for {:02x}", bd.bd_addr);
                        crate::ble::security::BOND_COMMANDS
                            .send(crate::ble::security::BondCommand::Store(bd))
                            .await;
                    }
                }
                trouble_host::prelude::ConnectionEvent::PairingFailed(err) => {
                    error!("Pairing failed: {:?}", err);
                }
                _ => {}
            }
        }
    };

    // Usually conn_fut finishes first, on disconnect.
    use embassy_futures::select::select4;
    let _ = select4(tx_fut, ctrl_rx_fut, data_rx_fut, conn_fut).await;
}
