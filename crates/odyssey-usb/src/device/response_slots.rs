use std::sync::Weak;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::packets::vm::{PacketData, PacketType};

#[derive(Default)]
pub(super) enum ResponseChannel {
    #[default]
    None,
    /// (generation, sender); see `ResponseSlot::generation`.
    Oneshot(u64, oneshot::Sender<PacketData>),
    Stream(mpsc::Sender<PacketData>),
    /// A request that timed out after being sent; its id stays reserved until a
    /// late reply is consumed or [`ABANDONED_ID_QUARANTINE`] passes.
    Abandoned(tokio::time::Instant),
}

/// How long an id whose request timed out is withheld from reuse.
pub(super) const ABANDONED_ID_QUARANTINE: Duration = Duration::from_secs(30);

impl ResponseChannel {
    pub(super) fn is_free(&self, now: tokio::time::Instant) -> bool {
        match self {
            Self::None => true,
            Self::Abandoned(since) => {
                now.saturating_duration_since(*since) >= ABANDONED_ID_QUARANTINE
            }
            Self::Oneshot(..) | Self::Stream(_) => false,
        }
    }
}

pub(super) struct State {
    // id 255 is reserved for requests that don't care for a response
    pub(super) response_channels: parking_lot::Mutex<[ResponseChannel; 255]>,
    pub(super) streams_active: StreamsActive,
    pub(super) next_generation: AtomicU64,
}

/// A helper struct to deal with cancellation
pub(super) struct ResponseSlot {
    pub(super) thread_state: Weak<State>,
    pub(super) id: u8,
    pub(super) finished: bool,
    /// Whether the request packet was handed to the transport.
    pub(super) sent: bool,
    /// Identifies the oneshot this slot registered (0 for stream slots), so a
    /// stale slot cannot clear a newer request that reused the id.
    pub(super) generation: u64,
}

impl Drop for ResponseSlot {
    fn drop(&mut self) {
        // If the future holding the slot was cancelled, release its channel.
        if !self.finished
            && let Some(thread_state) = self.thread_state.upgrade()
        {
            let mut chans = thread_state.response_channels.lock();
            let slot = &mut chans[usize::from(self.id)];
            // The dispatcher takes a oneshot sender out of its slot before waking
            // the receiver, so a request cancelled by a timeout right after its
            // reply was delivered can find the id already handed to a newer
            // request. Only touch the oneshot this slot registered.
            let ours = match slot {
                ResponseChannel::Oneshot(generation, _) => *generation == self.generation,
                ResponseChannel::Stream(_) => true,
                ResponseChannel::None | ResponseChannel::Abandoned(_) => false,
            };
            if ours {
                // Replies carry only the 8-bit id, so once a request has hit the
                // wire its id must not be reused while its reply may still be in
                // flight: quarantine it. A request that never left the host has
                // nothing in flight and frees the id immediately.
                *slot = if self.sent && matches!(slot, ResponseChannel::Oneshot(..)) {
                    ResponseChannel::Abandoned(tokio::time::Instant::now())
                } else {
                    ResponseChannel::None
                };
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct StreamsActive([AtomicBool; 256]);

impl Default for StreamsActive {
    fn default() -> Self {
        Self(std::array::from_fn(|_| AtomicBool::new(false)))
    }
}

impl std::ops::Index<PacketType> for StreamsActive {
    type Output = AtomicBool;

    fn index(&self, index: PacketType) -> &Self::Output {
        &self.0[u8::from(index) as usize]
    }
}

impl std::ops::IndexMut<PacketType> for StreamsActive {
    fn index_mut(&mut self, index: PacketType) -> &mut Self::Output {
        &mut self.0[u8::from(index) as usize]
    }
}
