use std::sync::Arc;
use std::time::Duration;

use odyssey_protocol::accessory::{
    Command, Completion, Event, MAX_PENDING_REQUESTS, REQUEST_TIMEOUT_MILLIS, Reply, Request,
};
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::packets::mux::MuxMsg;
use crate::{Error, Result};

const EVENT_CAPACITY: usize = 128;
const REQUEST_TIMEOUT: Duration = Duration::from_millis(REQUEST_TIMEOUT_MILLIS);
type CompletionResult = std::result::Result<Reply, odyssey_protocol::accessory::Error>;

#[derive(Debug)]
struct Pending {
    id: u32,
    reply: oneshot::Sender<CompletionResult>,
}

#[derive(Debug)]
struct State {
    next_id: u32,
    pending: [Option<Pending>; MAX_PENDING_REQUESTS],
    events: Option<broadcast::Sender<Event>>,
}

#[derive(Debug)]
pub(super) struct AccessoryDispatcher {
    state: Mutex<State>,
    pub(super) cancel: CancellationToken,
}

impl AccessoryDispatcher {
    pub(super) fn new(cancel: CancellationToken) -> Arc<Self> {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Arc::new(Self {
            state: Mutex::new(State {
                next_id: 0,
                pending: std::array::from_fn(|_| None),
                events: Some(events),
            }),
            cancel,
        })
    }

    pub(super) fn subscribe(&self) -> broadcast::Receiver<Event> {
        if let Some(events) = &self.state.lock().events {
            events.subscribe()
        } else {
            let (_, receiver) = broadcast::channel(1);
            receiver
        }
    }

    pub(super) fn complete(&self, completion: Completion) {
        let mut state = self.state.lock();
        if let Some(slot) = state.pending.iter_mut().find(|slot| {
            slot.as_ref()
                .is_some_and(|pending| pending.id == completion.id)
        }) && let Some(pending) = slot.take()
        {
            let _ = pending.reply.send(completion.result);
        }
    }

    pub(super) fn event(&self, event: Event) {
        if let Some(events) = &self.state.lock().events {
            let _ = events.send(event);
        }
    }

    pub(super) fn close(&self) {
        let mut state = self.state.lock();
        state.events.take();
        for slot in &mut state.pending {
            slot.take();
        }
    }

    fn reserve(self: &Arc<Self>) -> Result<(Reservation, oneshot::Receiver<CompletionResult>)> {
        let mut state = self.state.lock();
        if self.cancel.is_cancelled() || state.events.is_none() {
            return Err(Error::Closed);
        }
        let index = state
            .pending
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Busy)?;
        // Never reuse an id: a late completion cannot satisfy a newer request.
        let id = state.next_id.checked_add(1).ok_or(Error::Busy)?;
        state.next_id = id;
        let (reply, receiver) = oneshot::channel();
        state.pending[index] = Some(Pending { id, reply });
        Ok((
            Reservation {
                dispatcher: Arc::clone(self),
                id,
            },
            receiver,
        ))
    }

    pub(super) async fn request(
        self: &Arc<Self>,
        writer: &mpsc::Sender<MuxMsg>,
        command: Command,
    ) -> Result<Reply> {
        let (reservation, reply) = self.reserve()?;
        let operation = async {
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Err(Error::Closed),
                result = writer.send(MuxMsg::AccessoryRequest(Request { id: reservation.id, command })) => {
                    result.map_err(|_closed| Error::Closed)?;
                }
            }
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => Err(Error::Closed),
                result = reply => result.map_err(|_closed| Error::Closed)?.map_err(Error::Accessory),
            }
        };
        tokio::time::timeout(REQUEST_TIMEOUT, operation)
            .await
            .map_err(|_elapsed| Error::Timeout(REQUEST_TIMEOUT))?
    }
}

struct Reservation {
    dispatcher: Arc<AccessoryDispatcher>,
    id: u32,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self.dispatcher.state.lock();
        if let Some(slot) = state
            .pending
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|pending| pending.id == self.id))
        {
            slot.take();
        }
    }
}

// Two attributes: clippy only treats a plain `cfg(test)` module as test code.
#[cfg(test)]
#[cfg(feature = "sim")]
mod tests;
