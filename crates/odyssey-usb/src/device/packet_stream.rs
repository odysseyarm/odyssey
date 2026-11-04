use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};

use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tracing::warn;

use super::response_slots::{NO_REPLY_ID, ResponseSlot};
use super::retry::{STREAM_CONTROL_ATTEMPT_TIMEOUT, STREAM_CONTROL_ATTEMPTS, retry};
use crate::packets::vm::{Packet, PacketData, PacketType, StreamUpdate, StreamUpdateAction};

/// The reports of one stream type. Dropping it tells the device to stop
/// sending them.
#[derive(Debug)]
pub struct PacketStream {
    pub(super) slot: ResponseSlot,
    pub(super) stream_type: PacketType,
    pub(super) sender: mpsc::Sender<Packet>,
    pub(super) receiver: ReceiverStream<PacketData>,
}

impl Stream for PacketStream {
    type Item = PacketData;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.get_mut().receiver).poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.receiver.size_hint()
    }
}

impl Drop for PacketStream {
    fn drop(&mut self) {
        let Some(thread_state) = self.slot.state() else {
            return;
        };
        thread_state.streams_active[self.stream_type].store(false, Ordering::Relaxed);
        // Send Disable packet for this specific stream
        let sender = self.sender.clone();
        let stream_type = self.stream_type;
        // Detached: the stream is already gone, so nothing can wait on the
        // outcome; a failure is logged below.
        tokio::spawn(async move {
            let disable = async || {
                sender
                    .send(Packet {
                        id: NO_REPLY_ID,
                        data: PacketData::StreamUpdate(StreamUpdate {
                            packet_id: stream_type,
                            action: StreamUpdateAction::Disable,
                        }),
                    })
                    .await
            };
            match retry(
                disable,
                STREAM_CONTROL_ATTEMPT_TIMEOUT,
                STREAM_CONTROL_ATTEMPTS,
            )
            .await
            {
                Some(Ok(())) => {}
                Some(Err(e)) => warn!("Failed to send Disable packet for {stream_type:?}, {e}"),
                None => warn!("Failed to send Disable packet for {stream_type:?}"),
            }
        });
    }
}
