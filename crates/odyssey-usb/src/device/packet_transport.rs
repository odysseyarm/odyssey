use std::sync::Arc;

use anyhow::Result;
use nusb::io::{EndpointRead, EndpointWrite};
use nusb::transfer::Bulk;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info, trace, warn};

use super::mux_device::MuxDevice;
use crate::packets::vm::Packet;

pub struct PacketTransport {
    pub(super) writer: mpsc::Sender<Packet>,
    pub(super) incoming_rx: mpsc::Receiver<Packet>,
    pub(super) cancel: Arc<tokio_util::sync::CancellationToken>,
}

#[derive(Clone, Debug)]
pub struct PacketTransportTx {
    pub(super) writer: mpsc::Sender<Packet>,
}

impl PacketTransportTx {
    #[inline]
    pub async fn send(&self, pkt: Packet) -> Result<(), mpsc::error::SendError<Packet>> {
        self.writer.send(pkt).await
    }
}

impl PacketTransport {
    pub fn usb(mut in_ep: EndpointRead<Bulk>, mut out_ep: EndpointWrite<Bulk>) -> Self {
        let (writer, mut writer_rx) = mpsc::channel::<Packet>(64);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Packet>(128);

        let cancel_token = tokio_util::sync::CancellationToken::new();

        tokio::spawn(async move {
            let mut raw = Vec::with_capacity(1024);
            while let Some(pkt) = writer_rx.recv().await {
                raw.clear();
                if let Err(e) = postcard::to_io(&pkt, &mut raw) {
                    error!("postcard serialize failed: {e}");
                    continue;
                }
                if let Err(e) = out_ep.write_all(&raw).await {
                    error!("usb write_all failed: {e}");
                    continue;
                }
                if let Err(e) = out_ep.flush_end_async().await {
                    error!("usb flush_end failed: {e:?}");
                    continue;
                }
            }
            info!("usb writer exits");
        });

        tokio::spawn(async move {
            let mut reader = in_ep.until_short_packet();
            let mut io_errs = 0u8;
            loop {
                let mut buf = Vec::with_capacity(256);
                match reader.read_to_end(&mut buf).await {
                    Err(e) => {
                        use std::io::ErrorKind::{
                            BrokenPipe, ConnectionReset, TimedOut, UnexpectedEof, WouldBlock,
                        };
                        match e.kind() {
                            TimedOut | WouldBlock => {
                                io_errs = 0;
                                continue;
                            }
                            BrokenPipe | UnexpectedEof | ConnectionReset => {
                                error!("mux usb read fatal error: {e:?} (kind: {:?})", e.kind());
                                break;
                            }
                            _ => {
                                if io_errs < 3 {
                                    warn!("usb read error (ignored): {e}");
                                    io_errs += 1;
                                    continue;
                                }
                                error!("usb read error: {e}");
                                break;
                            }
                        }
                    }
                    Ok(_) => {
                        io_errs = 0;
                    }
                }
                if let Err(e) = reader.consume_end() {
                    warn!("usb consume_end failed: {e:?}");
                    continue;
                }
                match postcard::from_bytes::<Packet>(&buf) {
                    Ok(pkt) => {
                        if incoming_tx.send(pkt).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => error!("postcard decode failed: {e:?}"),
                }
            }
            info!("usb reader exits");
        });

        Self {
            writer,
            incoming_rx,
            cancel: Arc::new(cancel_token),
        }
    }

    /// Create a PacketTransport that routes VM packets through a MuxDevice
    pub fn mux(mux: MuxDevice, device_addr: [u8; 6]) -> Self {
        use rand::Rng as _;
        let transport_id: u32 = rand::thread_rng().r#gen();
        debug!(
            "PacketTransport::mux() [ID:{}] called for device {:02X}:{:02X}...",
            transport_id, device_addr[0], device_addr[1]
        );
        let (writer, mut writer_rx) = mpsc::channel::<Packet>(64);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Packet>(128);

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_reader = cancel_token.clone();

        // Register this device's packet channel
        {
            let mut channels = mux.device_packets_tx.lock().unwrap();
            channels.insert(device_addr, incoming_tx.clone());
            debug!(
                "mux: [ID:{}] registered packet channel for device {:02X}:{:02X}...",
                transport_id, device_addr[0], device_addr[1]
            );
        }

        // Clone what we need before moving mux
        let device_packets_tx = Arc::clone(&mux.device_packets_tx);

        // Writer task: wraps outgoing VM packets in MuxMsg::SendTo
        let transport_id_writer = transport_id;
        tokio::spawn(async move {
            debug!("mux writer task started [ID:{}]", transport_id_writer);
            while let Some(pkt) = writer_rx.recv().await {
                trace!(
                    "mux writer: sending packet to device {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                    device_addr[0],
                    device_addr[1],
                    device_addr[2],
                    device_addr[3],
                    device_addr[4],
                    device_addr[5]
                );
                if let Err(e) = mux.send_to(device_addr, pkt).await {
                    error!("mux send_to failed: {e}");
                    break;
                }
            }
            debug!("mux writer task exits [ID:{}]", transport_id_writer);
        });

        // Cleanup task - unregister when cancelled
        let transport_id_cleanup = transport_id;
        tokio::spawn(async move {
            cancel_reader.cancelled().await;
            let mut channels = device_packets_tx.lock().unwrap();
            channels.remove(&device_addr);
            debug!(
                "mux: [ID:{}] unregistered packet channel for device {:02X}:{:02X}...",
                transport_id_cleanup, device_addr[0], device_addr[1]
            );
        });

        Self {
            writer,
            incoming_rx,
            cancel: Arc::new(cancel_token),
        }
    }

    pub async fn send(&self, pkt: Packet) -> Result<(), mpsc::error::SendError<Packet>> {
        self.writer.send(pkt).await
    }

    pub fn split(
        self,
    ) -> (
        PacketTransportTx,
        ReceiverStream<Packet>,
        Arc<tokio_util::sync::CancellationToken>,
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
