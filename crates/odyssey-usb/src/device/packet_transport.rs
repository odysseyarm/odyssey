use std::sync::Arc;

use nusb::io::{EndpointRead, EndpointWrite};
use nusb::transfer::Bulk;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, trace};

use super::hex_address::HexAddress;
use super::mux_device::MuxDevice;
use super::mux_transport::{DevicePacketRoute, register_route, unregister_route};
use super::usb_bulk_io::{MessageReader, spawn_writer};
use crate::packets::vm::Packet;

/// Packets queued towards the device before `send` waits.
const OUTGOING_CAPACITY: usize = 64;
/// Packets queued from the device before the reader waits.
const INCOMING_CAPACITY: usize = 128;

/// A link that carries vision-module packets, either straight over USB or
/// tunnelled through a dongle.
#[derive(Debug)]
pub struct PacketTransport {
    pub(super) writer: mpsc::Sender<Packet>,
    pub(super) incoming_rx: mpsc::Receiver<Packet>,
    pub(super) cancel: Arc<CancellationToken>,
}

/// The sending half of a [`PacketTransport`].
#[derive(Clone, Debug)]
pub struct PacketTransportTx {
    pub(super) writer: mpsc::Sender<Packet>,
}

impl PacketTransportTx {
    #[inline]
    #[expect(
        clippy::result_large_err,
        reason = "SendError<Packet> hands the unsent packet back and is part of the public API"
    )]
    pub async fn send(&self, pkt: Packet) -> Result<(), mpsc::error::SendError<Packet>> {
        self.writer.send(pkt).await
    }
}

impl PacketTransport {
    pub fn usb(in_ep: EndpointRead<Bulk>, out_ep: EndpointWrite<Bulk>) -> Self {
        let (writer, writer_rx) = mpsc::channel::<Packet>(OUTGOING_CAPACITY);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Packet>(INCOMING_CAPACITY);

        let cancel = CancellationToken::new();
        spawn_writer(out_ep, writer_rx, "usb", cancel.clone());

        // Ends when the endpoint fails or every receiver of `incoming_rx` is
        // gone; the dispatcher then sees the stream close and fails its requests.
        let mut reader = MessageReader::new(in_ep, "usb");
        let reader_cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                let pkt = tokio::select! {
                    () = reader_cancel.cancelled() => break,
                    () = incoming_tx.closed() => break,
                    packet = reader.next_message::<Packet>() => match packet {
                        Some(packet) => packet,
                        None => break,
                    },
                };
                tokio::select! {
                    () = reader_cancel.cancelled() => break,
                    result = incoming_tx.send(pkt) => if result.is_err() { break; },
                }
            }
            reader_cancel.cancel();
            tracing::info!("usb reader exits");
        });

        Self {
            writer,
            incoming_rx,
            cancel: Arc::new(cancel),
        }
    }

    /// Create a PacketTransport that routes VM packets through a MuxDevice
    pub fn mux(mux: MuxDevice, device_addr: [u8; 6]) -> Self {
        use rand::Rng as _;
        let transport_id: u32 = rand::thread_rng().r#gen();
        let address = HexAddress(device_addr);
        debug!("PacketTransport::mux() [ID:{transport_id}] called for device {address}");
        let (writer, mut writer_rx) = mpsc::channel::<Packet>(OUTGOING_CAPACITY);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Packet>(INCOMING_CAPACITY);

        let cancel_token = CancellationToken::new();
        let cancel_reader = cancel_token.clone();

        // Register this device's packet channel
        let registered_route = incoming_tx.downgrade();
        register_route(
            &mux.device_packets_tx,
            device_addr,
            DevicePacketRoute {
                sender: incoming_tx,
                cancel: cancel_token.clone(),
            },
        );
        debug!("mux: [ID:{transport_id}] registered packet channel for device {address}");

        let device_packets_tx = Arc::clone(&mux.device_packets_tx);

        // Writer task: wraps outgoing VM packets in MuxMsg::SendTo. Ends when
        // the transport is dropped or the dongle link fails.
        let cancel_writer = cancel_token.clone();
        tokio::spawn(async move {
            debug!("mux writer task started [ID:{transport_id}]");
            loop {
                let pkt = tokio::select! {
                    biased;
                    () = cancel_writer.cancelled() => break,
                    packet = writer_rx.recv() => match packet {
                        Some(packet) => packet,
                        None => break,
                    },
                };
                trace!("mux writer: sending packet to device {address}");
                let result = tokio::select! {
                    biased;
                    () = cancel_writer.cancelled() => break,
                    result = mux.send_to_transport(device_addr, pkt, &cancel_writer) => result,
                };
                if let Err(e) = result {
                    error!("mux send_to failed: {e}");
                    break;
                }
            }
            cancel_writer.cancel();
            debug!("mux writer task exits [ID:{transport_id}]");
        });

        // Cleanup task - unregister when cancelled
        tokio::spawn(async move {
            cancel_reader.cancelled().await;
            unregister_route(&device_packets_tx, device_addr, &registered_route);
            debug!("mux: [ID:{transport_id}] unregistered packet channel for device {address}");
        });

        Self {
            writer,
            incoming_rx,
            cancel: Arc::new(cancel_token),
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "SendError<Packet> hands the unsent packet back and is part of the public API"
    )]
    pub async fn send(&self, pkt: Packet) -> Result<(), mpsc::error::SendError<Packet>> {
        self.writer.send(pkt).await
    }

    pub fn split(
        self,
    ) -> (
        PacketTransportTx,
        ReceiverStream<Packet>,
        Arc<CancellationToken>,
    ) {
        let Self {
            writer,
            incoming_rx,
            cancel,
        } = self;
        let tx_only = PacketTransportTx { writer };
        (tx_only, ReceiverStream::new(incoming_rx), cancel)
    }
}
