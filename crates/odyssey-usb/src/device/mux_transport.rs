use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use heapless::Vec as HVec;
use nusb::io::{EndpointRead, EndpointWrite};
use nusb::transfer::Bulk;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, info, trace};

use super::hex_address::HexAddress;
use super::usb_bulk_io::{MessageReader, spawn_writer};
use crate::packets::mux::{MAX_DEVICES, MuxMsg};
use crate::packets::vm::Packet;

/// Messages queued towards the dongle before `send` waits.
const OUTGOING_CAPACITY: usize = 64;
/// Messages queued from the dongle, per kind, before the reader waits.
const INCOMING_CAPACITY: usize = 128;

/// Device addresses the dongle currently reports.
pub(super) type DeviceSnapshot = HVec<[u8; 6], MAX_DEVICES>;
/// Where packets from each paired device are delivered.
pub(super) type DevicePacketRoutes = Mutex<HashMap<[u8; 6], mpsc::Sender<Packet>>>;

/// The routing table is a plain map that is never left half-updated, so a
/// panic elsewhere while it was held does not invalidate it.
pub(super) fn lock_routes(
    routes: &DevicePacketRoutes,
) -> MutexGuard<'_, HashMap<[u8; 6], mpsc::Sender<Packet>>> {
    routes.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The link to a dongle: control messages, device snapshots, and the packets
/// of every device behind it, demultiplexed.
#[derive(Debug)]
pub struct MuxTransport {
    writer: mpsc::Sender<MuxMsg>,
    snapshots_rx: mpsc::Receiver<DeviceSnapshot>,
    msg_rx: mpsc::Receiver<MuxMsg>,
    device_packets_tx: Arc<DevicePacketRoutes>,
}

/// The parts of a [`MuxTransport`], see [`MuxTransport::split`].
pub(super) struct MuxTransportParts {
    pub(super) writer: mpsc::Sender<MuxMsg>,
    pub(super) snapshots: ReceiverStream<DeviceSnapshot>,
    pub(super) control: ReceiverStream<MuxMsg>,
    pub(super) device_packets_tx: Arc<DevicePacketRoutes>,
}

impl MuxTransport {
    pub fn usb(in_ep: EndpointRead<Bulk>, out_ep: EndpointWrite<Bulk>) -> Self {
        let (writer, writer_rx) = mpsc::channel::<MuxMsg>(OUTGOING_CAPACITY);
        let (snapshots_tx, snapshots_rx) = mpsc::channel::<DeviceSnapshot>(INCOMING_CAPACITY);
        let (msg_tx, msg_rx) = mpsc::channel::<MuxMsg>(INCOMING_CAPACITY);
        let device_packets_tx = Arc::new(DevicePacketRoutes::default());
        let routes = Arc::clone(&device_packets_tx);

        spawn_writer(out_ep, writer_rx, "mux usb");

        // Routes each message by kind. Ends when the endpoint fails.
        let mut reader = MessageReader::new(in_ep, "mux usb");
        tokio::spawn(async move {
            while let Some(msg) = reader.next_message::<MuxMsg>().await {
                match msg {
                    MuxMsg::DevicesSnapshot(devices) => {
                        debug!("mux reader: routing DevicesSnapshot");
                        let _ = snapshots_tx.send(devices.0).await;
                    }
                    MuxMsg::DevicePacket(dev_pkt) => {
                        trace!(
                            "mux reader: routing DevicePacket for {}",
                            HexAddress(dev_pkt.dev)
                        );
                        // Clone the sender to avoid holding the lock across await
                        let route = lock_routes(&routes).get(&dev_pkt.dev).cloned();
                        match route {
                            Some(tx) => {
                                let _ = tx.send(dev_pkt.pkt).await;
                            }
                            None => trace!(
                                "mux reader: no receiver registered for device {}",
                                HexAddress(dev_pkt.dev)
                            ),
                        }
                    }
                    other => {
                        debug!("mux reader: routing control message");
                        let _ = msg_tx.send(other).await;
                    }
                }
            }
            info!("mux usb reader exits");
        });

        Self {
            writer,
            snapshots_rx,
            msg_rx,
            device_packets_tx,
        }
    }

    pub(super) fn split(self) -> MuxTransportParts {
        let Self {
            writer,
            snapshots_rx,
            msg_rx,
            device_packets_tx,
        } = self;
        MuxTransportParts {
            writer,
            snapshots: ReceiverStream::new(snapshots_rx),
            control: ReceiverStream::new(msg_rx),
            device_packets_tx,
        }
    }
}
