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
    Stream(u64, PacketType, mpsc::Sender<PacketData>),
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
            Self::Oneshot(..) | Self::Stream(..) => false,
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
    pub(super) stream_control: tokio::sync::Mutex<()>,
    closed: AtomicBool,
    next_generation: AtomicU64,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            response_channels: parking_lot::Mutex::new(std::array::from_fn(|_| {
                ResponseChannel::None
            })),
            streams_active: StreamsActive::default(),
            stream_control: tokio::sync::Mutex::new(()),
            closed: AtomicBool::new(false),
            next_generation: AtomicU64::new(1),
        }
    }

    /// Reserves the lowest free control id for a request that expects one reply.
    pub(super) fn reserve_oneshot(
        self: &Arc<Self>,
    ) -> Option<(ResponseSlot, oneshot::Receiver<PacketData>)> {
        let mut channels = self.response_channels.lock();
        if self.closed.load(Ordering::Relaxed) {
            return None;
        }
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

    /// Reserves a stream id until its guard drops or a global reset closes it.
    pub(super) fn reserve_stream(
        self: &Arc<Self>,
        buffer: usize,
        stream_type: PacketType,
    ) -> Option<(ResponseSlot, mpsc::Receiver<PacketData>)> {
        let mut channels = self.response_channels.lock();
        if self.closed.load(Ordering::Relaxed) {
            return None;
        }
        let (id, channel) = channels
            .iter_mut()
            .enumerate()
            .skip(STREAMING_ID_MIN)
            .find(|(_, channel)| matches!(channel, ResponseChannel::None))?;
        let (send, receiver) = mpsc::channel(buffer);
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        *channel = ResponseChannel::Stream(generation, stream_type, send);
        Some((ResponseSlot::new(self, id, generation), receiver))
    }

    /// Close subscriptions after DisableAll was acknowledged. Old guards keep
    /// their generation, so dropping them cannot stop replacement subscriptions.
    pub(super) fn clear_streams(&self) {
        let mut channels = self.response_channels.lock();
        for channel in channels.iter_mut() {
            if let ResponseChannel::Stream(_, stream_type, _) = channel {
                self.streams_active[*stream_type].store(false, Ordering::Relaxed);
                *channel = ResponseChannel::None;
            }
        }
    }

    /// Fail pending requests even if an in-progress operation still holds State.
    pub(super) fn close(&self) {
        let mut channels = self.response_channels.lock();
        self.closed.store(true, Ordering::Relaxed);
        for channel in channels.iter_mut() {
            if let ResponseChannel::Stream(_, stream_type, _) = channel {
                self.streams_active[*stream_type].store(false, Ordering::Relaxed);
            }
            *channel = ResponseChannel::None;
        }
    }

    /// Hands a reply to whatever is waiting on its id.
    pub(super) fn deliver(&self, reply: Packet) -> Delivery {
        let mut channels = self.response_channels.lock();
        let Some(slot) = channels.get_mut(usize::from(reply.id)) else {
            // 255 is the no-reply id, not an index into the response table.
            return Delivery::Unregistered;
        };
        let data = reply.data;
        match std::mem::take(slot) {
            ResponseChannel::None => Delivery::Unregistered,
            // Its id was quarantined precisely so this cannot reach a newer request.
            ResponseChannel::Abandoned(_) => Delivery::LateReplyDropped,
            ResponseChannel::Oneshot(_, tx) => match tx.send(data) {
                Ok(()) => Delivery::Delivered,
                Err(e) => Delivery::Undeliverable(format!("{e:?}")),
            },
            ResponseChannel::Stream(generation, stream_type, tx) => {
                let outcome = match tx.try_send(data) {
                    Ok(()) => Delivery::Delivered,
                    Err(e) => Delivery::Undeliverable(format!("{e:?}")),
                };
                *slot = ResponseChannel::Stream(generation, stream_type, tx);
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
    /// Identifies the reservation, so an old guard cannot clear a newer request
    /// or a stream that reused the id after clear_all_streams.
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

    /// Run a non-blocking action while this stream reservation is still current.
    /// Keeping the table locked orders Disable enqueueing against stream reset.
    pub(super) fn with_current_stream<T>(&self, action: impl FnOnce() -> T) -> Option<T> {
        let state = self.thread_state.upgrade()?;
        let channels = state.response_channels.lock();
        matches!(
            &channels[usize::from(self.id)],
            ResponseChannel::Stream(generation, ..) if *generation == self.generation
        )
        .then(action)
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
            // request. Only touch the reservation this guard registered.
            let ours = match slot {
                ResponseChannel::Oneshot(generation, _)
                | ResponseChannel::Stream(generation, ..) => *generation == self.generation,
                ResponseChannel::None | ResponseChannel::Abandoned(_) => false,
            };
            if ours {
                if let ResponseChannel::Stream(_, stream_type, _) = slot {
                    thread_state.streams_active[*stream_type].store(false, Ordering::Relaxed);
                }
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
