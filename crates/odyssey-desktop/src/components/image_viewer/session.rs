use std::collections::VecDeque;
use std::time::Instant;

use dioxus::prelude::*;
use futures::StreamExt as _;
use odyssey_hub_common::device::Device;
use odyssey_protocol::Port;
use odyssey_protocol::image::ImageStreamConfig;

use super::frame::PresentedFrame;
use crate::hub::HubContext;

/// The component lifetime is the task lifetime. Dioxus cancels use_future on
/// unmount, including while Start is pending; dropping the dedicated RPC stream
/// tells the hub to stop pixels. Legacy tracking still requires reboot/reconnect.
#[component]
pub(super) fn LiveSession(
    hub: Signal<HubContext>,
    device: Device,
    camera: Port,
    fps: u8,
    mut latest: Signal<Option<PresentedFrame>>,
    mut status: Signal<String>,
    mut error: Signal<Option<String>>,
    mut running: Signal<bool>,
) -> Element {
    use_future(move || {
        let device = device.clone();
        let client = hub.peek().client();
        async move {
            let legacy = matches!(device.product_id, 0x520f | 0x5210);
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
                    error.set(Some(format!("Could not start USB image capture: {cause}")));
                    status.set(if legacy { "No preview active. Legacy startup may have changed sensor mode: reboot/power-cycle and reconnect before tracking." } else { "Stopped; no image session is active." }.to_owned());
                    running.set(false);
                    return;
                }
            };
            let mut arrivals = VecDeque::with_capacity(31);
            status.set(
                if legacy {
                    "Capturing legacy USB images. Tracking is stopped until reboot/reconnect."
                } else {
                    "Capturing USB images. Tracking paused until Stop or close."
                }
                .to_owned(),
            );
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
                        match PresentedFrame::new(frame, received_at, observed_fps) {
                            Ok(frame) => latest.set(Some(frame)),
                            Err(cause) => break format!("Could not display image: {cause}"),
                        }
                    }
                    Some(Err(cause)) => break format!("USB image stream failed: {cause}"),
                    None => {
                        break "USB image stream ended. The device may have disconnected."
                            .to_owned();
                    }
                }
            };
            // Release the session before exposing a restart button.
            drop(stream);
            error.set(Some(failure));
            status.set(if legacy { "Pixels stopped. Legacy tracking is NOT restored; reboot/power-cycle and reconnect." } else { "Stopped. Any displayed image is the last received frame." }.to_owned());
            running.set(false);
        }
    });
    rsx! {}
}
