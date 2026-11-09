use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use heapless::Vec as HVec;
use nusb::io::{EndpointRead, EndpointWrite};
use nusb::transfer::Bulk;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::{CancellationToken, DropGuard};
use tracing::{debug, info, trace};

use super::hex_address::HexAddress;
use super::mux_accessory::AccessoryDispatcher;
use super::usb_bulk_io::{MessageReader, spawn_writer};
use crate::packets::mux::{MAX_DEVICES, MuxMsg, Version};
use crate::packets::vm::Packet;

/// Messages queued towards the dongle before `send` waits.
const OUTGOING_CAPACITY: usize = 64;
/// Messages queued from the dongle, per kind, before the reader waits.
const INCOMING_CAPACITY: usize = 128;

/// Device addresses the dongle currently reports.
pub(super) type DeviceSnapshot = HVec<[u8; 6], MAX_DEVICES>;
/// Both halves of a VM route have one owner. Replacing its incoming channel
/// also cancels the old outgoing task, including any queued stream cleanup.
#[derive(Debug)]
pub(super) struct DevicePacketRoute {
    pub(super) sender: mpsc::Sender<Packet>,
    pub(super) cancel: CancellationToken,
}

pub(super) type DevicePacketRoutes = Mutex<HashMap<[u8; 6], DevicePacketRoute>>;

/// The routing table is a plain map that is never left half-updated, so a
/// panic elsewhere while it was held does not invalidate it.
pub(super) fn lock_routes(
    routes: &DevicePacketRoutes,
) -> MutexGuard<'_, HashMap<[u8; 6], DevicePacketRoute>> {
    routes.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) fn register_route(
    routes: &DevicePacketRoutes,
    address: [u8; 6],
    route: DevicePacketRoute,
) {
    let mut routes = lock_routes(routes);
    if let Some(previous) = routes.insert(address, route) {
        previous.cancel.cancel();
    }
}

pub(super) fn unregister_route(
    routes: &DevicePacketRoutes,
    address: [u8; 6],
    registered: &mpsc::WeakSender<Packet>,
) {
    let mut routes = lock_routes(routes);
    if let Some(registered) = registered.upgrade()
        && routes
            .get(&address)
            .is_some_and(|route| route.sender.same_channel(&registered))
    {
        routes.remove(&address);
    }
}

/// The link to a dongle: control messages, device snapshots, and the packets
/// of every device behind it, demultiplexed.
#[derive(Debug)]
pub struct MuxTransport {
    writer: mpsc::Sender<MuxMsg>,
    snapshots_rx: mpsc::Receiver<DeviceSnapshot>,
    msg_rx: mpsc::Receiver<MuxMsg>,
    accessories: Arc<AccessoryDispatcher>,
    versions: watch::Receiver<Option<Version>>,
    device_packets_tx: Arc<DevicePacketRoutes>,
    lifetime: Arc<DropGuard>,
}

/// The parts of a [`MuxTransport`], see [`MuxTransport::split`].
pub(super) struct MuxTransportParts {
    pub(super) writer: mpsc::Sender<MuxMsg>,
    pub(super) snapshots: ReceiverStream<DeviceSnapshot>,
    pub(super) control: ReceiverStream<MuxMsg>,
    pub(super) accessories: Arc<AccessoryDispatcher>,
    pub(super) versions: watch::Receiver<Option<Version>>,
    pub(super) device_packets_tx: Arc<DevicePacketRoutes>,
    pub(super) lifetime: Arc<DropGuard>,
}

impl MuxTransport {
    pub fn usb(in_ep: EndpointRead<Bulk>, out_ep: EndpointWrite<Bulk>) -> Self {
        let (writer, writer_rx) = mpsc::channel::<MuxMsg>(OUTGOING_CAPACITY);
        let (snapshots_tx, snapshots_rx) = mpsc::channel::<DeviceSnapshot>(INCOMING_CAPACITY);
        let (msg_tx, msg_rx) = mpsc::channel::<MuxMsg>(INCOMING_CAPACITY);
        let device_packets_tx = Arc::new(DevicePacketRoutes::default());
        let routes = Arc::clone(&device_packets_tx);

        let cancel = CancellationToken::new();
        let accessories = AccessoryDispatcher::new(cancel.clone());
        let reader_accessories = Arc::clone(&accessories);
        let (versions_tx, versions) = watch::channel(None);
        spawn_writer(out_ep, writer_rx, "mux usb", cancel.clone());
        let lifetime = Arc::new(cancel.clone().drop_guard());

        // Routes each message by kind. Ends when the endpoint fails.
        let mut reader = MessageReader::new(in_ep, "mux usb");
        tokio::spawn(async move {
            loop {
                let msg = tokio::select! {
                    () = cancel.cancelled() => break,
                    message = reader.next_message::<MuxMsg>() => match message {
                        Some(message) => message,
                        None => break,
                    },
                };
                let route_message = route_message(
                    msg,
                    &snapshots_tx,
                    &msg_tx,
                    &routes,
                    &reader_accessories,
                    &versions_tx,
                );
                tokio::select! {
                    () = cancel.cancelled() => break,
                    () = route_message => {}
                }
            }
            cancel.cancel();
            reader_accessories.close();
            for (_, route) in lock_routes(&routes).drain() {
                route.cancel.cancel();
            }
            info!("mux usb reader exits");
        });

        Self {
            writer,
            snapshots_rx,
            msg_rx,
            accessories,
            versions,
            device_packets_tx,
            lifetime,
        }
    }

    /// Opens a typed in-process dongle link, using the same routing as USB.
    #[cfg(feature = "sim")]
    pub fn from_channels(
        writer: mpsc::Sender<MuxMsg>,
        mut incoming: mpsc::Receiver<MuxMsg>,
    ) -> Self {
        let (snapshots_tx, snapshots_rx) = mpsc::channel(INCOMING_CAPACITY);
        let (msg_tx, msg_rx) = mpsc::channel(INCOMING_CAPACITY);
        let device_packets_tx = Arc::new(DevicePacketRoutes::default());
        let routes = Arc::clone(&device_packets_tx);
        let cancel = CancellationToken::new();
        let accessories = AccessoryDispatcher::new(cancel.clone());
        let reader_accessories = accessories.clone();
        let (versions_tx, versions) = watch::channel(None);
        let lifetime = Arc::new(cancel.clone().drop_guard());
        tokio::spawn(async move {
            loop {
                let msg = tokio::select! {
                    () = cancel.cancelled() => break,
                    msg = incoming.recv() => match msg {
                        Some(msg) => msg,
                        None => break,
                    },
                };
                tokio::select! {
                    () = cancel.cancelled() => break,
                    () = route_message(msg, &snapshots_tx, &msg_tx, &routes, &reader_accessories, &versions_tx) => {}
                }
            }
            cancel.cancel();
            reader_accessories.close();
            for (_, route) in lock_routes(&routes).drain() {
                route.cancel.cancel();
            }
        });
        Self {
            writer,
            snapshots_rx,
            msg_rx,
            accessories,
            versions,
            device_packets_tx,
            lifetime,
        }
    }

    pub(super) fn split(self) -> MuxTransportParts {
        let Self {
            writer,
            snapshots_rx,
            msg_rx,
            accessories,
            versions,
            device_packets_tx,
            lifetime,
        } = self;
        MuxTransportParts {
            writer,
            snapshots: ReceiverStream::new(snapshots_rx),
            control: ReceiverStream::new(msg_rx),
            accessories,
            versions,
            device_packets_tx,
            lifetime,
        }
    }
}

async fn route_message(
    msg: MuxMsg,
    snapshots_tx: &mpsc::Sender<DeviceSnapshot>,
    msg_tx: &mpsc::Sender<MuxMsg>,
    routes: &DevicePacketRoutes,
    accessories: &AccessoryDispatcher,
    versions: &watch::Sender<Option<Version>>,
) {
    match msg {
        MuxMsg::AccessoryCompletion(completion) => accessories.complete(completion),
        MuxMsg::AccessoryEvent(event) => accessories.event(event),
        MuxMsg::AccessoryRequest(_) => {
            trace!("mux reader: ignoring host-directed accessory request");
        }
        MuxMsg::ReadVersionResponse(version) => {
            versions.send_replace(Some(version));
        }
        MuxMsg::DevicesSnapshot(devices) => {
            debug!("mux reader: routing DevicesSnapshot");
            let _ = snapshots_tx.send(devices.0).await;
        }
        MuxMsg::DevicePacket(dev_pkt) => {
            trace!(
                "mux reader: routing DevicePacket for {}",
                HexAddress(dev_pkt.dev)
            );
            // A full replaced route must not pin the reader or keep the old
            // transport's incoming stream open.
            let route = lock_routes(routes)
                .get(&dev_pkt.dev)
                .map(|route| (route.sender.clone(), route.cancel.clone()));
            match route {
                Some((tx, route_cancel)) => {
                    tokio::select! {
                        biased;
                        () = route_cancel.cancelled() => {}
                        _ = tx.send(dev_pkt.pkt) => {}
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::vm::PacketData;

    #[tokio::test]
    async fn replacing_route_closes_old_link_and_stale_cleanup_keeps_new_link() {
        let routes = DevicePacketRoutes::default();
        let address = [1, 2, 3, 4, 5, 6];
        let (old_sender, mut old_receiver) = mpsc::channel(1);
        let old_registration = old_sender.downgrade();
        let old_cancel = CancellationToken::new();
        register_route(
            &routes,
            address,
            DevicePacketRoute {
                sender: old_sender,
                cancel: old_cancel.clone(),
            },
        );
        let (new_sender, mut new_receiver) = mpsc::channel(1);
        let new_cancel = CancellationToken::new();
        register_route(
            &routes,
            address,
            DevicePacketRoute {
                sender: new_sender,
                cancel: new_cancel.clone(),
            },
        );
        assert!(old_cancel.is_cancelled());
        assert!(old_receiver.recv().await.is_none());
        unregister_route(&routes, address, &old_registration);
        lock_routes(&routes)
            .get(&address)
            .unwrap()
            .sender
            .try_send(Packet {
                id: 17,
                data: PacketData::Ack(),
            })
            .unwrap();
        assert!(matches!(
            new_receiver.recv().await,
            Some(Packet {
                id: 17,
                data: PacketData::Ack()
            })
        ));
        assert!(!new_cancel.is_cancelled());
    }
}
