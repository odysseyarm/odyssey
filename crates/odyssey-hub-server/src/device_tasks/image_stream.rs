//! A single replaceable frame slot for the dedicated image RPC. Slow consumers see
//! the newest complete frame, never an unbounded pixel backlog or partial frames.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::Stream;
use futures::task::AtomicWaker;
use odyssey_hub_server_interface::ImageFrame;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tonic::Status;

#[derive(Default)]
struct State {
    latest: Option<ImageFrame>,
    finished: bool,
    error: Option<Status>,
}

struct Shared {
    state: Mutex<State>,
    waker: AtomicWaker,
    closed: CancellationToken,
}

pub(crate) struct ImageDelivery(Arc<Shared>);

impl std::fmt::Debug for ImageResponseStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageResponseStream")
            .finish_non_exhaustive()
    }
}
pub(crate) struct ImageResponseStream(Arc<Shared>);

impl ImageDelivery {
    pub(crate) fn channel() -> (Self, ImageResponseStream) {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            waker: AtomicWaker::new(),
            closed: CancellationToken::new(),
        });
        (Self(Arc::clone(&shared)), ImageResponseStream(shared))
    }

    pub(crate) fn publish(&self, frame: ImageFrame) {
        self.0.state.lock().latest = Some(frame);
        self.0.waker.wake();
    }

    pub(crate) fn finish(&self, error: Option<Status>) {
        let mut state = self.0.state.lock();
        state.finished = true;
        state.error = error;
        drop(state);
        self.0.waker.wake();
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.0.closed.is_cancelled()
    }

    pub(crate) async fn closed(&self) {
        self.0.closed.cancelled().await;
    }
}

impl Drop for ImageDelivery {
    fn drop(&mut self) {
        self.0.state.lock().finished = true;
        self.0.waker.wake();
    }
}

impl Stream for ImageResponseStream {
    type Item = Result<ImageFrame, Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Register before inspecting the slot so publication cannot race a Pending.
        self.0.waker.register(cx.waker());
        let mut state = self.0.state.lock();
        if let Some(frame) = state.latest.take() {
            Poll::Ready(Some(Ok(frame)))
        } else if state.finished {
            Poll::Ready(state.error.take().map(Err))
        } else {
            Poll::Pending
        }
    }
}

impl Drop for ImageResponseStream {
    fn drop(&mut self) {
        self.0.closed.cancel();
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt as _;

    use super::*;

    #[tokio::test]
    async fn slow_consumer_gets_newest_frame_then_terminal_transport_error() {
        let (delivery, mut stream) = ImageDelivery::channel();
        delivery.publish(ImageFrame {
            frame_id: 1,
            pixels: vec![1; 98 * 98],
            width: 98,
            height: 98,
            ..Default::default()
        });
        delivery.publish(ImageFrame {
            frame_id: 2,
            pixels: vec![2; 98 * 98],
            width: 98,
            height: 98,
            ..Default::default()
        });
        delivery.finish(Some(Status::unavailable("USB disconnected")));
        drop(delivery);
        let frame = stream.next().await.unwrap().unwrap();
        assert_eq!(frame.frame_id, 2);
        assert_eq!(frame.pixels, vec![2; 98 * 98]);
        assert_eq!(
            stream.next().await.unwrap().unwrap_err().code(),
            tonic::Code::Unavailable
        );
        assert!(stream.next().await.is_none());
    }
}
