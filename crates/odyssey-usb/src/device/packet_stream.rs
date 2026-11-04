use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tracing::warn;

use super::response_slots::{NO_REPLY_ID, ResponseSlot};
use crate::packets::vm::{Packet, PacketData, PacketType, StreamUpdate, StreamUpdateAction};

/// The reports of one stream type. Dropping it tells the device to stop
/// sending them.
#[derive(Debug)]
pub struct PacketStream {
    pub(super) slot: Option<ResponseSlot>,
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
        let Some(slot) = self.slot.take() else {
            return;
        };
        let disable = Packet {
            id: NO_REPLY_ID,
            data: PacketData::StreamUpdate(StreamUpdate {
                packet_id: self.stream_type,
                action: StreamUpdateAction::Disable,
            }),
        };
        // Keep the type reserved until Disable is queued. Otherwise an
        // immediate replacement can queue Enable before our detached Disable
        // and be stopped by the old subscription.
        let Some(Some(disable)) =
            slot.with_current_stream(|| match self.sender.try_send(disable) {
                Err(mpsc::error::TrySendError::Full(packet)) => Some(packet),
                Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => None,
            })
        else {
            return;
        };
        let sender = self.sender.clone();
        let stream_type = self.stream_type;
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            warn!("Cannot queue Disable for {stream_type:?} without a runtime");
            return;
        };
        // Retain the slot until Disable has a place in the queue. A global
        // reset may supersede it while we wait; recheck the generation before
        // enqueueing so this cleanup cannot disable a replacement.
        runtime.spawn(async move {
            match sender.reserve().await {
                Ok(permit) => {
                    let _ = slot.with_current_stream(|| permit.send(disable));
                }
                Err(e) => warn!("Failed to send Disable packet for {stream_type:?}, {e}"),
            }
        });
    }
}
