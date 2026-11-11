use std::collections::VecDeque;
use std::time::Instant;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{Context, Task, Window};
use odyssey_hub_client::client::Client;
use odyssey_hub_common::device::Device;
use odyssey_protocol::Port;
use odyssey_protocol::image::ImageStreamConfig;
use tokio::task::JoinHandle;

use super::dialog::ImageDialog;
use super::frame::PresentedFrame;

/// What the stream task tells the dialog.
pub(super) enum SessionEvent {
    Started,
    StartFailed(String),
    Frame(PresentedFrame),
    /// The stream is over, with the reason to show. The RPC is already released.
    Ended(String),
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The dialog owns the session, so its lifetime is the stream's lifetime.
/// Dropping it aborts the task, including while Start is pending, and
/// dropping the dedicated RPC stream tells the hub to stop pixels. Legacy
/// tracking still requires reboot/reconnect.
pub(super) struct LiveSession {
    _stream: AbortOnDrop,
    _pump: Task<()>,
}

impl LiveSession {
    pub(super) fn start(
        client: Option<Client>,
        device: Device,
        camera: Port,
        fps: u8,
        window: &mut Window,
        cx: &mut Context<ImageDialog>,
    ) -> Self {
        let (events, mut received) = unbounded();
        let stream = crate::runtime::spawn(stream_images(client, device, camera, fps, events));
        let pump = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = received.next().await {
                let delivered = this.update_in(cx, |dialog, window, cx| {
                    dialog.on_session_event(event, window, cx);
                });
                if delivered.is_err() {
                    break;
                }
            }
        });
        Self {
            _stream: AbortOnDrop(stream),
            _pump: pump,
        }
    }
}

async fn stream_images(
    client: Option<Client>,
    device: Device,
    camera: Port,
    fps: u8,
    events: UnboundedSender<SessionEvent>,
) {
    let started = match client {
        Some(client) => client
            .device(device)
            .stream_images(ImageStreamConfig { camera, fps })
            .await
            .map_err(|cause| cause.to_string()),
        None => Err("not connected to the hub".to_owned()),
    };
    let mut stream = match started {
        Ok(stream) => stream,
        Err(cause) => {
            let _ = events.unbounded_send(SessionEvent::StartFailed(cause));
            return;
        }
    };
    if events.unbounded_send(SessionEvent::Started).is_err() {
        return;
    }
    let mut arrivals = VecDeque::with_capacity(31);
    let failure = loop {
        match stream.next().await {
            Some(Ok(frame)) => {
                let received_at = Instant::now();
                arrivals.push_back(received_at);
                while arrivals.len() > 30
                    || (arrivals.len() > 2
                        && received_at.duration_since(arrivals[0]).as_secs_f64() > 2.0)
                {
                    arrivals.pop_front();
                }
                let observed_fps = if arrivals.len() > 1 {
                    let elapsed = received_at.duration_since(arrivals[0]).as_secs_f64();
                    (elapsed > 0.0).then(|| (arrivals.len() - 1) as f64 / elapsed)
                } else {
                    None
                };
                match PresentedFrame::new(&frame, received_at, observed_fps) {
                    Ok(frame) => {
                        // The dialog is gone, so there is nothing left to stream for.
                        if events.unbounded_send(SessionEvent::Frame(frame)).is_err() {
                            return;
                        }
                    }
                    Err(cause) => break format!("Could not display image: {cause}"),
                }
            }
            Some(Err(cause)) => break format!("USB image stream failed: {cause}"),
            None => {
                break "USB image stream ended. The device may have disconnected.".to_owned();
            }
        }
    };
    // Release the session before exposing a restart button.
    drop(stream);
    let _ = events.unbounded_send(SessionEvent::Ended(failure));
}
