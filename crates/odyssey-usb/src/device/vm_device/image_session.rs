//! The dispatcher supervises this future alongside packet delivery. It is the
//! sole owner of image sessions and never detaches cancellation cleanup tasks.
use std::sync::{Arc, Weak};

use odyssey_protocol::image::{ImageCommand, ImageStatusCode, ImageStreamConfig};
use odyssey_protocol::{Packet, PacketData, PacketType};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::super::packet_transport::PacketTransportTx;
use super::State;
use crate::image::{ImageStream, Reassembler};
use crate::{Error, Result};

// Bounded startup plus uncertain-start cleanup fits the hub's control deadline.
const IMAGE_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

pub(super) struct ImageRequest {
    pub config: ImageStreamConfig,
    pub cancel: CancellationToken,
    pub ready: oneshot::Sender<Result<ImageStream>>,
    pub _keepalive: Arc<(tokio_util::sync::DropGuard, tokio_util::sync::DropGuard)>,
}

pub(super) async fn supervise(
    transport: PacketTransportTx,
    state: Weak<State>,
    mut requests: mpsc::Receiver<ImageRequest>,
    shutdown: CancellationToken,
) {
    let mut unsupported = false;
    // A random initial generation avoids collisions after reconnecting to a
    // unit which still has a session from a previous host process.
    let mut generation = rand::random::<u32>();
    loop {
        let request = tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            request = requests.recv() => match request { Some(request) => request, None => break },
        };
        if request.cancel.is_cancelled() || request.ready.is_closed() {
            continue;
        }
        if unsupported {
            let _ = request.ready.send(Err(Error::ImageUnsupported));
            continue;
        }
        generation = generation.wrapping_add(1);
        let Some(state) = state.upgrade() else {
            let _ = request.ready.send(Err(Error::Closed));
            break;
        };
        let (slot, mut packets) = match state.reserve_stream(64, PacketType::ImageChunk()) {
            Ok(slot) => slot,
            Err(error) => {
                let _ = request.ready.send(Err(error));
                continue;
            }
        };
        let start = async {
            transport
                .send(Packet {
                    id: slot.id,
                    data: PacketData::ImageCommand(ImageCommand::Start {
                        generation,
                        config: request.config,
                    }),
                })
                .await
                .map_err(|_closed| Error::Closed)?;
            loop {
                match packets.recv().await {
                    Some(PacketData::ImageStatus(status)) if status.generation != generation => {}
                    Some(PacketData::ImageChunk(chunk)) if chunk.generation != generation => {}
                    Some(PacketData::ImageObject(object)) if object.generation != generation => {}
                    Some(PacketData::ImageStatus(status)) => {
                        return match status.status {
                            ImageStatusCode::Started => Ok(()),
                            ImageStatusCode::Unsupported => Err(Error::ImageUnsupported),
                            ImageStatusCode::UsbRequired => Err(Error::ImageUsbRequired),
                            other => Err(Error::ImageStatus(other)),
                        };
                    }
                    Some(PacketData::Ack()) => return Err(Error::ImageUnsupported),
                    Some(other) => return Err(Error::unexpected(&other)),
                    None => return Err(Error::Closed),
                }
            }
        };
        let started = tokio::select! {
            biased;
            () = shutdown.cancelled() => Err(Error::Closed),
            () = request.cancel.cancelled() => Err(Error::Closed),
            result = tokio::time::timeout(IMAGE_COMMAND_TIMEOUT, start) => result.unwrap_or(Err(Error::Timeout(IMAGE_COMMAND_TIMEOUT))),
        };
        if let Err(error) = started {
            // Release ingress before requesting Stop: otherwise a final USB
            // image burst could fill the queue ahead of the control response.
            drop(packets);
            unsupported = matches!(error, Error::ImageUnsupported);
            // Only an uncertain/cancelled startup can have enabled capture.
            // A truthful refusal never needs a second request to old firmware.
            if !matches!(
                error,
                Error::ImageUnsupported | Error::ImageUsbRequired | Error::ImageStatus(_)
            ) && let Err(error) = stop(&transport, &state, generation).await
            {
                tracing::debug!(?error, "image startup cleanup");
            }
            let _ = request.ready.send(Err(error));
            continue;
        }
        let (done, stopped) = oneshot::channel();
        let (stream, frames) = ImageStream::channel(request.cancel.clone(), stopped);
        let delivered = request.ready.send(Ok(stream)).is_ok();
        let mut assembler = Reassembler::new(generation, request.config.camera);
        let mut accepts_competitors = true;
        if delivered {
            loop {
                tokio::select! {
                    biased;
                    () = shutdown.cancelled() => { frames.send(Err(Error::Closed)); break; },
                    () = request.cancel.cancelled() => break,
                    // Do not queue competing physical sessions behind a stream
                    // whose consumer may keep it open indefinitely.
                    competing = requests.recv(), if accepts_competitors => match competing {
                        Some(competing) => { let _ = competing.ready.send(Err(Error::ImageStatus(ImageStatusCode::Busy))); },
                        None => accepts_competitors = false,
                    },
                    () = frames.renewal_requested() => {
                        let renew = Packet { id: slot.id, data: PacketData::ImageCommand(ImageCommand::Renew { generation }) };
                        if transport.send(renew).await.is_err() { frames.send(Err(Error::Closed)); break; }
                    },
                    packet = packets.recv() => match packet {
                        Some(PacketData::ImageChunk(chunk)) => match assembler.chunk(&chunk) {
                            Ok(Some(frame)) => frames.send(Ok(frame)),
                            Ok(None) => {},
                            Err(error) => { frames.send(Err(error)); break; },
                        },
                        Some(PacketData::ImageObject(object)) => if let Err(error) = assembler.object(object) {
                            frames.send(Err(error));
                            break;
                        },
                        Some(PacketData::ImageStatus(status)) if status.generation == generation => {
                            frames.send(Err(Error::ImageStatus(status.status)));
                            break;
                        },
                        Some(_) => {},
                        None => { frames.send(Err(Error::Closed)); break; },
                    }
                }
            }
        }
        drop(packets);
        // The stream slot stays reserved until Stop is acknowledged. Late data
        // is discarded with this session and cannot reach a replacement.
        let result = stop(&transport, &state, generation).await;
        drop(slot);
        drop(frames);
        let _ = done.send(result);
    }
}

async fn stop(transport: &PacketTransportTx, state: &Arc<State>, generation: u32) -> Result<()> {
    tokio::time::timeout(IMAGE_COMMAND_TIMEOUT, async {
        let (mut slot, reply) = state.reserve_oneshot()?;
        transport
            .send(Packet {
                id: slot.id,
                data: PacketData::ImageCommand(ImageCommand::Stop { generation }),
            })
            .await
            .map_err(|_closed| Error::Closed)?;
        slot.sent = true;
        let reply = reply.await.map_err(|_closed| Error::Closed)?;
        slot.finished = true;
        match reply {
            PacketData::ImageStatus(status) if status.generation == generation => {
                match status.status {
                    ImageStatusCode::Stopped => Ok(()),
                    ImageStatusCode::RebootRequired => {
                        Err(Error::ImageStatus(ImageStatusCode::RebootRequired))
                    }
                    _ => Err(Error::ImageStatus(status.status)),
                }
            }
            other => Err(Error::unexpected(&other)),
        }
    })
    .await
    .map_err(|_elapsed| Error::Timeout(IMAGE_COMMAND_TIMEOUT))?
}

#[cfg(test)]
mod tests;
