//! HAL-independent OTA request/reply service. A board library supplies the flash backend.
//!
//! One exchange at a time: both channels hold one item, and each request waits for its reply.

#![no_std]

use better_ota::{Action, Receiver, Slots};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use odyssey_protocol::ota::{MAX_FRAME, STATUS_LEN};

const QUEUE: usize = 1;

static REQUESTS: Channel<ThreadModeRawMutex, Frame, QUEUE> = Channel::new();
static REPLIES: Channel<ThreadModeRawMutex, Reply, QUEUE> = Channel::new();

#[derive(Clone, Copy)]
struct Frame {
    len: u16,
    bytes: [u8; MAX_FRAME],
}

/// Status for the hub, and whether to reset after the reply is sent.
#[derive(Clone, Copy, Debug)]
pub struct Reply {
    /// Encoded `odyssey_protocol::ota::Status`.
    pub status: [u8; STATUS_LEN],
    /// A commit succeeded; the caller resets the device once the reply has gone out.
    pub reset: bool,
}

/// Sends OTA frames to [`run`]. Every copy talks to the same task.
#[derive(Clone, Copy, Debug, Default)]
pub struct OtaHandle;

impl OtaHandle {
    /// Returns the reply to `bytes`, or `None` if the frame is over [`MAX_FRAME`].
    pub async fn request(&self, bytes: &[u8]) -> Option<Reply> {
        let Ok(len) = u16::try_from(bytes.len()) else {
            return None;
        };
        if bytes.len() > MAX_FRAME {
            return None;
        }
        let mut frame = Frame {
            len,
            bytes: [0; MAX_FRAME],
        };
        frame.bytes[..bytes.len()].copy_from_slice(bytes);
        REQUESTS.send(frame).await;
        Some(REPLIES.receive().await)
    }
}

/// Owns the receiver for the life of the firmware. `app_slot` and `supported` are reported in
/// every status; `netcore_release` is read per reply because a query to the net core can finish
/// after boot.
pub async fn run(
    mut slots: impl Slots,
    app_slot: u8,
    supported: u8,
    firmware_version: [u32; 3],
    netcore_release: fn() -> u16,
) -> ! {
    let mut receiver = Receiver::new();
    loop {
        let frame = REQUESTS.receive().await;
        let input = &frame.bytes[..usize::from(frame.len)];
        let action = receiver.handle(&mut slots, input).await;
        let status = receiver.status(app_slot, supported, firmware_version, netcore_release());
        REPLIES
            .send(Reply {
                status: status.encode(),
                reset: matches!(action, Ok(Action::Reset)),
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::pin::pin;
    use core::task::{Context, Poll, Waker};

    use odyssey_protocol::ota::MAX_FRAME;

    use super::OtaHandle;

    #[test]
    fn oversized_frames_fail_without_contacting_worker() {
        let handle = OtaHandle;
        let bytes = [0; MAX_FRAME + 1];
        let mut request = pin!(handle.request(&bytes));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(
            request.as_mut().poll(&mut context),
            Poll::Ready(None)
        ));
    }
}
