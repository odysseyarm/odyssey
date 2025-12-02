use core::sync::atomic::Ordering;
use defmt::{error, info, warn};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use protodongers::mux::DevicePacket;
use trouble_host::prelude::{BdAddr, L2capChannel};
use trouble_host::{PacketPool, Stack};

use super::DevicePacketChannel;
use super::central::{
    ActiveConnections, DISCONNECT_COUNT, DROPPED_DATA_TO_HOST, DeviceQueue, DeviceQueues,
    is_control_packet,
};

/// Connection handler task - runs in background for each connected device
#[embassy_executor::task(pool_size = 3)]
pub async fn connection_handler_task(
    conn: trouble_host::prelude::Connection<'static, crate::Pool>,
    l2cap_control_channel: L2capChannel<'static, crate::Pool>,
    l2cap_data_channel: L2capChannel<'static, crate::Pool>,
    device_queue: &'static DeviceQueue,
    device_packets: &'static DevicePacketChannel,
    active_connections: &'static ActiveConnections,
    device_queues: &'static DeviceQueues,
    stack: &'static Stack<'static, crate::Controller, crate::Pool>,
    target: BdAddr,
) {
    info!("Connection handler task started for {:02x}", target);

    // Split L2CAP channels into separate readers and writers
    let (mut control_writer, mut control_reader) = l2cap_control_channel.split();
    let (mut data_writer, mut data_reader) = l2cap_data_channel.split();

    // Cooperative shutdown signal
    let shutdown = Signal::<ThreadModeRawMutex, ()>::new();

    // TX future - sends packets from queue to device
    let tx_fut = async {
        let mut tx_buf = [0u8; 1024];
        loop {
            if embassy_time::with_timeout(Duration::from_millis(0), shutdown.wait())
                .await
                .is_ok()
            {
                return;
            }
            let device_pkt = match device_queue.try_receive() {
                Ok(pkt) => pkt,
                Err(_) => {
                    Timer::after(Duration::from_micros(200)).await;
                    continue;
                }
            };
            let is_control = is_control_packet(&device_pkt.pkt);
            if let Ok(data) = postcard::to_slice(&device_pkt.pkt, &mut tx_buf) {
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
                        error!("L2CAP send failed: {:?}", defmt::Debug2Format(&e));
                        shutdown.signal(());
                        return;
                    }
                    Err(_) => {
                        warn!("L2CAP send timeout; will retry next packet");
                    }
                }
                let elapsed = Instant::now() - start;
                if elapsed >= Duration::from_millis(100) {
                    warn!("L2CAP send took {} ms", elapsed.as_millis());
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
            if embassy_time::with_timeout(Duration::from_millis(0), shutdown.wait())
                .await
                .is_ok()
            {
                return;
            }
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
            if embassy_time::with_timeout(Duration::from_millis(0), shutdown.wait())
                .await
                .is_ok()
            {
                return;
            }
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

    // Connection event future
    let conn_fut = async {
        loop {
            match conn.next().await {
                trouble_host::prelude::ConnectionEvent::Disconnected { reason } => {
                    let c = DISCONNECT_COUNT.fetch_add(1, Ordering::AcqRel) + 1;
                    info!("Disconnected: {:?} (total={})", reason, c);
                    shutdown.signal(());
                    return;
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

    // Run all futures until one signals shutdown
    use embassy_futures::join::join;
    let _ = join(join(tx_fut, ctrl_rx_fut), join(data_rx_fut, conn_fut)).await;

    // Cleanup
    info!(
        "Connection handler ending for device {:02x}, cleaning up",
        target
    );
    active_connections.remove(&target.into_inner()).await;
    device_queues.unregister(&target.into_inner()).await;
}
