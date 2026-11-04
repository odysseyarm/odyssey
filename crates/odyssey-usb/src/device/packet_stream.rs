use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::task::Poll;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tracing::warn;

use super::response_slots::ResponseSlot;
use super::retry::retry;
use crate::packets::vm::{Packet, PacketData, PacketType, StreamUpdate};

pub struct PacketStream {
    pub(super) slot: ResponseSlot,
    pub(super) stream_type: PacketType,
    pub(super) sender: mpsc::Sender<Packet>,
    pub(super) receiver: ReceiverStream<PacketData>,
}

impl Stream for PacketStream {
    type Item = PacketData;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.get_mut().receiver).poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.receiver.size_hint()
    }
}

impl Drop for PacketStream {
    fn drop(&mut self) {
        if let Some(thread_state) = self.slot.thread_state.upgrade() {
            thread_state.streams_active[self.stream_type].store(false, Ordering::Relaxed);
            // Send Disable packet for this specific stream
            let sender = self.sender.clone();
            let stream_type = self.stream_type;
            tokio::spawn(async move {
                match retry(
                    async || {
                        sender
                            .send(Packet {
                                id: 255,
                                data: PacketData::StreamUpdate(StreamUpdate {
                                    packet_id: stream_type,
                                    action: crate::packets::vm::StreamUpdateAction::Disable,
                                }),
                            })
                            .await
                    },
                    Duration::from_millis(2000),
                    5,
                )
                .await
                {
                    Some(Ok(())) => {}
                    Some(Err(e)) => {
                        warn!("Failed to send Disable packet for {:?}, {}", stream_type, e);
                    }
                    None => {
                        warn!("Failed to send Disable packet for {:?}", stream_type);
                    }
                }
            });
        }
    }
}
