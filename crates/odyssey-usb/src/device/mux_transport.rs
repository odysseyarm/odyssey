use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use heapless::Vec as HVec;
use nusb::io::{EndpointRead, EndpointWrite};
use nusb::transfer::Bulk;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info, trace, warn};

use crate::packets::mux::{MAX_DEVICES, MuxMsg};
use crate::packets::vm::Packet;

pub struct MuxTransport {
    writer: mpsc::Sender<MuxMsg>,
    snapshots_rx: mpsc::Receiver<HVec<[u8; 6], MAX_DEVICES>>,
    msg_rx: mpsc::Receiver<MuxMsg>,
    device_packets_tx: Arc<Mutex<HashMap<[u8; 6], mpsc::Sender<Packet>>>>,
}

impl MuxTransport {
    pub fn usb(mut in_ep: EndpointRead<Bulk>, mut out_ep: EndpointWrite<Bulk>) -> Self {
        let (writer, mut writer_rx) = mpsc::channel::<MuxMsg>(64);
        let (snapshots_tx, snapshots_rx) = mpsc::channel::<HVec<[u8; 6], MAX_DEVICES>>(128);
        let (msg_tx, msg_rx) = mpsc::channel::<MuxMsg>(128);
        let device_packets_tx =
            Arc::new(Mutex::new(HashMap::<[u8; 6], mpsc::Sender<Packet>>::new()));
        let device_packets_tx_clone = Arc::clone(&device_packets_tx);

        // Writer task
        tokio::spawn(async move {
            let mut raw = Vec::with_capacity(1024);
            while let Some(msg) = writer_rx.recv().await {
                raw.clear();
                if let Err(e) = postcard::to_io(&msg, &mut raw) {
                    error!("mux postcard serialize failed: {e}");
                    continue;
                }
                if let Err(e) = out_ep.write_all(&raw).await {
                    error!("mux usb write_all failed: {e}");
                    continue;
                }
                if let Err(e) = out_ep.flush_end_async().await {
                    error!("mux usb flush_end failed: {e:?}");
                    continue;
                }
            }
            info!("mux usb writer exits");
        });

        // Reader task - routes messages by type
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
                                    warn!("mux usb read error (ignored): {e}");
                                    io_errs += 1;
                                    continue;
                                }
                                error!("mux usb read error: {e}");
                                break;
                            }
                        }
                    }
                    Ok(_) => {
                        io_errs = 0;
                    }
                }
                if let Err(e) = reader.consume_end() {
                    warn!("mux usb consume_end failed: {e:?}");
                    continue;
                }
                match postcard::from_bytes::<MuxMsg>(&buf) {
                    Ok(MuxMsg::DevicesSnapshot(devices)) => {
                        debug!("mux reader: routing DevicesSnapshot");
                        let _ = snapshots_tx.send(devices.0).await;
                    }
                    Ok(MuxMsg::DevicePacket(dev_pkt)) => {
                        trace!(
                            "mux reader: routing DevicePacket for {:02X}:{:02X}...",
                            dev_pkt.dev[0], dev_pkt.dev[1]
                        );
                        // Clone the sender to avoid holding the lock across await
                        let tx_opt = {
                            let channels = device_packets_tx_clone.lock().unwrap();
                            channels.get(&dev_pkt.dev).cloned()
                        };
                        if let Some(tx) = tx_opt {
                            let _ = tx.send(dev_pkt.pkt).await;
                        } else {
                            trace!(
                                "mux reader: no receiver registered for device {:02X}:{:02X}...",
                                dev_pkt.dev[0], dev_pkt.dev[1]
                            );
                        }
                    }
                    Ok(other) => {
                        debug!("mux reader: routing control message");
                        let _ = msg_tx.send(other).await;
                    }
                    Err(e) => error!("mux postcard decode failed: {e:?}"),
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

    pub fn split(
        self,
    ) -> (
        mpsc::Sender<MuxMsg>,
        ReceiverStream<HVec<[u8; 6], MAX_DEVICES>>,
        ReceiverStream<MuxMsg>,
        Arc<Mutex<HashMap<[u8; 6], mpsc::Sender<Packet>>>>,
    ) {
        let Self {
            writer,
            snapshots_rx,
            msg_rx,
            device_packets_tx,
        } = self;
        (
            writer,
            ReceiverStream::new(snapshots_rx),
            ReceiverStream::new(msg_rx),
            device_packets_tx,
        )
    }
}
