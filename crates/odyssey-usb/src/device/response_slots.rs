//! Bookkeeping that matches a reply to the request that is waiting for it.
//!
//! Replies carry only an 8-bit request id, so every in-flight request owns one
//! slot, and the dispatcher routes a reply to whatever occupies the slot with
//! its id.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::packets::vm::{Packet, PacketData, PacketType};

/// Id of a request that wants no reply, so it never occupies a slot.
pub(super) const NO_REPLY_ID: u8 = 255;
/// One slot per usable id: everything below [`NO_REPLY_ID`].
const SLOT_COUNT: usize = NO_REPLY_ID as usize;
/// Ids `0..=CONTROL_ID_MAX` serve request/response pairs. Keeping them apart
/// from the stream ids means a burst of long-lived streams cannot starve
/// ordinary requests of an id.
const CONTROL_ID_MAX: usize = 207;
/// Ids from here up serve streams (47 of them, below [`NO_REPLY_ID`]).
const STREAMING_ID_MIN: usize = 208;
/// One flag per possible packet type byte.
const PACKET_TYPE_COUNT: usize = 256;

/// How long an id whose request timed out is withheld from reuse.
pub(super) const ABANDONED_ID_QUARANTINE: Duration = Duration::from_secs(30);

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

impl ResponseChannel {
    fn is_free(&self, now: tokio::time::Instant) -> bool {
        match self {
            Self::None => true,
            Self::Abandoned(since) => {
                now.saturating_duration_since(*since) >= ABANDONED_ID_QUARANTINE
            }
            Self::Oneshot(..) | Self::Stream(_) => false,
        }
    }
}

/// What the dispatcher did with one reply.
#[derive(Debug)]
pub(super) enum Delivery {
    Delivered,
    /// No request was waiting for this id.
    Unregistered,
    /// A late reply to a request that already timed out.
    LateReplyDropped,
    /// The waiting side is gone or full; the reply was discarded.
    Undeliverable(String),
}

pub(super) struct State {
    // id 255 is reserved for requests that don't care for a response
    pub(super) response_channels: parking_lot::Mutex<[ResponseChannel; SLOT_COUNT]>,
    pub(super) streams_active: StreamsActive,
    next_generation: AtomicU64,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            response_channels: parking_lot::Mutex::new(std::array::from_fn(|_| {
                ResponseChannel::None
            })),
            streams_active: StreamsActive::default(),
            next_generation: AtomicU64::new(1),
        }
    }

    /// Reserves the lowest free control id for a request that expects one reply.
    pub(super) fn reserve_oneshot(
        self: &Arc<Self>,
    ) -> Option<(ResponseSlot, oneshot::Receiver<PacketData>)> {
        let mut channels = self.response_channels.lock();
        let now = tokio::time::Instant::now();
        let (id, channel) = channels
            .iter_mut()
            .enumerate()
            .take(CONTROL_ID_MAX + 1)
            .find(|(_, channel)| channel.is_free(now))?;
        let (send, receiver) = oneshot::channel();
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        *channel = ResponseChannel::Oneshot(generation, send);
        Some((ResponseSlot::new(self, id, generation), receiver))
    }

    /// Reserves a stream id for replies that keep arriving until the slot drops.
    pub(super) fn reserve_stream(
        self: &Arc<Self>,
        buffer: usize,
    ) -> Option<(ResponseSlot, mpsc::Receiver<PacketData>)> {
        let mut channels = self.response_channels.lock();
        let (id, channel) = channels
            .iter_mut()
            .enumerate()
            .skip(STREAMING_ID_MIN)
            .find(|(_, channel)| matches!(channel, ResponseChannel::None))?;
        let (send, receiver) = mpsc::channel(buffer);
        *channel = ResponseChannel::Stream(send);
        // Stream slots never replace each other, so they need no generation.
        Some((ResponseSlot::new(self, id, 0), receiver))
    }

    /// Hands a reply to whatever is waiting on its id.
    pub(super) fn deliver(&self, reply: Packet) -> Delivery {
        let mut channels = self.response_channels.lock();
        let slot = &mut channels[usize::from(reply.id)];
        let data = reply.data;
        match std::mem::take(slot) {
            ResponseChannel::None => Delivery::Unregistered,
            // Its id was quarantined precisely so this cannot reach a newer request.
            ResponseChannel::Abandoned(_) => Delivery::LateReplyDropped,
            ResponseChannel::Oneshot(_, tx) => match tx.send(data) {
                Ok(()) => Delivery::Delivered,
                Err(e) => Delivery::Undeliverable(format!("{e:?}")),
            },
            ResponseChannel::Stream(tx) => {
                let outcome = match tx.try_send(data) {
                    Ok(()) => Delivery::Delivered,
                    Err(e) => Delivery::Undeliverable(format!("{e:?}")),
                };
                *slot = ResponseChannel::Stream(tx);
                outcome
            }
        }
    }
}

/// A helper struct to deal with cancellation
#[derive(Debug)]
pub(super) struct ResponseSlot {
    thread_state: Weak<State>,
    pub(super) id: u8,
    pub(super) finished: bool,
    /// Whether the request packet was handed to the transport.
    pub(super) sent: bool,
    /// Identifies the oneshot this slot registered (0 for stream slots), so a
    /// stale slot cannot clear a newer request that reused the id.
    generation: u64,
}

impl ResponseSlot {
    fn new(state: &Arc<State>, id: usize, generation: u64) -> Self {
        Self {
            thread_state: Arc::downgrade(state),
            // Slot indices are below `SLOT_COUNT`, which is below 256.
            id: u8::try_from(id).expect("slot index fits the 8-bit request id"),
            finished: false,
            sent: false,
            generation,
        }
    }

    /// The state of the device this slot belongs to, unless the device is gone.
    pub(super) fn state(&self) -> Option<Arc<State>> {
        self.thread_state.upgrade()
    }
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

/// Which stream types are currently enabled on a device, indexed by packet
/// type, so a second subscription to the same stream is refused.
#[derive(Debug)]
pub(super) struct StreamsActive([AtomicBool; PACKET_TYPE_COUNT]);

impl Default for StreamsActive {
    fn default() -> Self {
        Self(std::array::from_fn(|_| AtomicBool::new(false)))
    }
}

impl std::ops::Index<PacketType> for StreamsActive {
    type Output = AtomicBool;

    fn index(&self, index: PacketType) -> &Self::Output {
        &self.0[usize::from(u8::from(index))]
    }
}
