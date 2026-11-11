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

/// Concrete construction path, never inferred from a USB product name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LinkKind {
    Usb,
    Multiplexed,
    Simulated,
}

/// A link that carries vision-module packets, either straight over USB or
/// tunnelled through a dongle.
///
/// Since: Odyssey Desktop 0.6.0.
#[derive(Debug)]
pub struct PacketTransport {
    pub(super) writer: mpsc::Sender<Packet>,
    pub(super) incoming_rx: mpsc::Receiver<Packet>,
    pub(super) cancel: Arc<CancellationToken>,
    pub(super) kind: LinkKind,
}

/// The sending half of a [`PacketTransport`].
///
/// Since: Odyssey Desktop 0.6.0.
#[derive(Clone, Debug)]
pub struct PacketTransportTx {
    pub(super) writer: mpsc::Sender<Packet>,
}

impl PacketTransportTx {
    /// Sends packet through bounded USB transport.
    ///
    /// # Errors
    /// Returns the unsent packet when the transport receiver has closed.
    /// Since: Odyssey Desktop 0.6.0.
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
    /// Since: Odyssey Desktop 0.6.0.
    pub fn usb(in_ep: EndpointRead<Bulk>, out_ep: EndpointWrite<Bulk>) -> Self {
        let (writer, writer_rx) = mpsc::channel::<Packet>(OUTGOING_CAPACITY);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Packet>(INCOMING_CAPACITY);

        let cancel = CancellationToken::new();
        spawn_writer(out_ep, writer_rx, "usb", cancel.clone());

        // Ends when the endpoint fails or every receiver of `incoming_rx` is
        // gone. The dispatcher then sees the stream close and fails its requests.
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
            kind: LinkKind::Usb,
        }
    }

    /// A transport over in-process channels, for a simulated unit: what the host
    /// sends comes out of the receiver paired with `writer`, and what is put into
    /// the sender paired with `incoming_rx` arrives as if from the unit. Dropping
    /// that sender closes the link.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn from_channels(
        writer: mpsc::Sender<Packet>,
        incoming_rx: mpsc::Receiver<Packet>,
    ) -> Self {
        Self {
            writer,
            incoming_rx,
            cancel: Arc::new(CancellationToken::new()),
            kind: LinkKind::Simulated,
        }
    }

    /// A transport whose packets are tunnelled through `mux` to and from the
    /// device at `device_addr`.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn mux(mux: MuxDevice, device_addr: [u8; 6]) -> Self {
        use rand::Rng as _;
        let transport_id: u32 = rand::thread_rng().r#gen();
        let address = HexAddress(device_addr);
        debug!("PacketTransport::mux() [ID:{transport_id}] called for device {address}");
        let (writer, mut writer_rx) = mpsc::channel::<Packet>(OUTGOING_CAPACITY);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Packet>(INCOMING_CAPACITY);

        let cancel_token = CancellationToken::new();
        let cancel_reader = cancel_token.clone();

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

        // Removes the route once the transport is cancelled.
        tokio::spawn(async move {
            cancel_reader.cancelled().await;
            unregister_route(&device_packets_tx, device_addr, &registered_route);
            debug!("mux: [ID:{transport_id}] unregistered packet channel for device {address}");
        });

        Self {
            writer,
            incoming_rx,
            cancel: Arc::new(cancel_token),
            kind: LinkKind::Multiplexed,
        }
    }

    /// Sends packet through bounded USB transport.
    ///
    /// # Errors
    /// Returns the unsent packet when the transport receiver has closed.
    /// Since: Odyssey Desktop 0.6.0.
    #[expect(
        clippy::result_large_err,
        reason = "SendError<Packet> hands the unsent packet back and is part of the public API"
    )]
    pub async fn send(&self, pkt: Packet) -> Result<(), mpsc::error::SendError<Packet>> {
        self.writer.send(pkt).await
    }

    /// Since: Odyssey Desktop 0.6.0.
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
            ..
        } = self;
        let tx_only = PacketTransportTx { writer };
        (tx_only, ReceiverStream::new(incoming_rx), cancel)
    }
}
