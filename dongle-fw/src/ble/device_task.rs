use core::sync::atomic::Ordering;
use defmt::{error, info, warn};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use protodongers::mux::DevicePacket;
use trouble_host::Stack;
use trouble_host::prelude::{
    AddrKind, BdAddr, ConnectConfig, ConnectParams, CreditFlowPolicy, L2capChannel,
    L2capChannelConfig, PhySet, ScanConfig,
};

use super::DevicePacketChannel;
use super::central::{
    ActiveConnections, DISCONNECT_COUNT, DROPPED_DATA_TO_HOST, DeviceQueue, DeviceQueues,
    L2CAP_PSM_CONTROL, L2CAP_PSM_DATA, is_control_packet,
};

/// Per-device connection task - handles complete lifecycle for one device
/// Each task builds its own Central instance from the shared stack
#[embassy_executor::task(pool_size = protodongers::mux::MAX_DEVICES)]
pub async fn device_connection_task(
    stack: &'static Stack<'static, crate::Controller, crate::Pool>,
    device_packets: &'static DevicePacketChannel,
    active_connections: &'static ActiveConnections,
    device_queues: &'static DeviceQueues,
    settings: &'static crate::storage::Settings,
    target: BdAddr,
) {
    info!("Device task started for {:02x}", target);

    // Build our own Central instance for this device
    let host = stack.build();
    let mut central = host.central;

    loop {
        // Check if we're still supposed to be connecting to this device
        // (bond may have been cleared while we were disconnected)
        let targets = settings.get_scan_targets().await;
        if !targets.contains(&target.into_inner()) {
            info!(
                "Device {:02x} no longer in scan targets, stopping task",
                target
            );
            return;
        }

        // Connect to the device
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
        let connect_result =
            embassy_time::with_timeout(Duration::from_secs(10), central.connect(&config)).await;

        let conn = match connect_result {
            Ok(Ok(conn)) => {
                info!("Connected to {:02x}", target);
                conn
            }
            Ok(Err(e)) => {
                error!(
                    "Connection failed for {:02x}: {:?}",
                    target,
                    defmt::Debug2Format(&e)
                );
                Timer::after_secs(5).await;
                continue;
            }
            Err(_) => {
                error!("Connect timeout for {:02x}", target);
                Timer::after_secs(5).await;
                continue;
            }
        };

        // Handle pairing/encryption
        let has_bond = settings.get_bond(target).await.is_some();
        let _ = conn.set_bondable(true);

        if !has_bond {
            info!("No bond found, initiating pairing for {:02x}", target);
            if let Err(e) = conn.request_security() {
                error!("Failed to request security: {:?}", defmt::Debug2Format(&e));
                continue;
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
                                crate::ble::security::BOND_TO_STORE.signal(bd);
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
                    continue;
                }
            }
        } else {
            info!("Using existing bond for {:02x}", target);
            if let Err(e) = conn.request_security() {
                error!(
                    "Failed to request security for bonded device: {:?}",
                    defmt::Debug2Format(&e)
                );
                continue;
            }

            let security_result = embassy_time::with_timeout(Duration::from_secs(10), async {
                loop {
                    match conn.next().await {
                        trouble_host::prelude::ConnectionEvent::PairingComplete {
                            security_level,
                            ..
                        } => {
                            info!("Encryption established: {:?}", security_level);
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
                    continue;
                }
            }
        }

        // Request 2M PHY
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

        // Create L2CAP channels
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
            L2capChannel::create(stack, &conn, L2CAP_PSM_CONTROL, &l2cap_control_config),
        )
        .await
        {
            Ok(Ok(ch)) => ch,
            Ok(Err(e)) => {
                error!(
                    "Failed to create L2CAP control channel: {:?}",
                    defmt::Debug2Format(&e)
                );
                continue;
            }
            Err(_) => {
                error!("L2CAP control channel creation timeout");
                continue;
            }
        };

        info!(
            "Setting up L2CAP data channel with PSM {:04x}",
            L2CAP_PSM_DATA
        );
        let l2cap_data_channel = match embassy_time::with_timeout(
            Duration::from_secs(10),
            L2capChannel::create(stack, &conn, L2CAP_PSM_DATA, &l2cap_data_config),
        )
        .await
        {
            Ok(Ok(ch)) => ch,
            Ok(Err(e)) => {
                error!(
                    "Failed to create L2CAP data channel: {:?}",
                    defmt::Debug2Format(&e)
                );
                continue;
            }
            Err(_) => {
                error!("L2CAP data channel creation timeout");
                continue;
            }
        };

        // Register device queue
        let device_queue = match device_queues.register(&target.into_inner()).await {
            Some(queue) => queue,
            None => {
                error!("Failed to register device queue for {:02x}", target);
                continue;
            }
        };

        // Add to active connections
        if !active_connections.add(&target.into_inner()).await {
            error!("Failed to add device to active connections");
            device_queues.unregister(&target.into_inner()).await;
            continue;
        }

        info!(
            "Device {:02x} fully connected, starting communication",
            target
        );

        // Run connection handler
        run_connection(
            conn,
            l2cap_control_channel,
            l2cap_data_channel,
            device_queue,
            device_packets,
            stack,
            target,
        )
        .await;

        info!("Device {:02x} disconnected, cleaning up", target);

        // Cleanup
        active_connections.remove(&target.into_inner()).await;
        device_queues.unregister(&target.into_inner()).await;

        // Wait before reconnecting
        Timer::after_secs(5).await;
    }
}

/// Run the connection handler loop until disconnection
async fn run_connection(
    conn: trouble_host::prelude::Connection<'static, crate::Pool>,
    l2cap_control_channel: L2capChannel<'static, crate::Pool>,
    l2cap_data_channel: L2capChannel<'static, crate::Pool>,
    device_queue: &'static DeviceQueue,
    device_packets: &'static DevicePacketChannel,
    stack: &'static Stack<'static, crate::Controller, crate::Pool>,
    target: BdAddr,
) {
    let (mut control_writer, mut control_reader) = l2cap_control_channel.split();
    let (mut data_writer, mut data_reader) = l2cap_data_channel.split();

    let shutdown = Signal::<ThreadModeRawMutex, ()>::new();

    // TX future
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
            let is_control = is_control_packet(&device_pkt.pkt);

            if let Ok(data) = postcard::to_slice(&device_pkt.pkt, &mut tx_buf) {
                let ch = if is_control { "ctrl" } else { "data" };
                if matches!(
                    device_pkt.pkt.data,
                    protodongers::PacketData::WriteConfig(_)
                        | protodongers::PacketData::FlashSettings()
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

    // Control RX future
    let ctrl_rx_fut = async {
        let mut rx_buf = [0u8; 1024];
        loop {
            match control_reader.receive(stack, &mut rx_buf).await {
                Ok(len) => match postcard::from_bytes::<protodongers::Packet>(&rx_buf[..len]) {
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

    // Data RX future
    let data_rx_fut = async {
        let mut rx_buf = [0u8; 1024];
        loop {
            match data_reader.receive(stack, &mut rx_buf).await {
                Ok(len) => match postcard::from_bytes::<protodongers::Packet>(&rx_buf[..len]) {
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

    // Connection event future - returns disconnect reason
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
                        crate::ble::security::BOND_TO_STORE.signal(bd);
                    }
                }
                trouble_host::prelude::ConnectionEvent::PairingFailed(err) => {
                    error!("Pairing failed: {:?}", err);
                }
                _ => {}
            }
        }
    };

    // Run all futures until one completes (usually conn_fut when disconnected)
    use embassy_futures::select::select4;
    let _ = select4(tx_fut, ctrl_rx_fut, data_rx_fut, conn_fut).await;
}
