use std::time::Instant;

use base64::Engine as _;
use dioxus::prelude::*;
use image::ImageEncoder as _;
use image::codecs::png::PngEncoder;
use odyssey_protocol::image::ImageFrame;

use super::HELP;

/// Keep encoded pixels once instead of re-encoding on every receive-age tick.
pub(super) struct PresentedFrame {
    timestamp_us: u32,
    width: u16,
    height: u16,
    image_url: String,
    received_at: Instant,
    observed_fps: Option<f64>,
}

impl PresentedFrame {
    pub(super) fn new(
        frame: ImageFrame,
        received_at: Instant,
        observed_fps: Option<f64>,
    ) -> image::ImageResult<Self> {
        let mut png = Vec::new();
        PngEncoder::new(&mut png).write_image(
            &frame.pixels,
            u32::from(frame.width),
            u32::from(frame.height),
            image::ExtendedColorType::L8,
        )?;
        let mut image_url = String::from("data:image/png;base64,");
        base64::engine::general_purpose::STANDARD.encode_string(png, &mut image_url);
        Ok(Self {
            timestamp_us: frame.timestamp_us,
            width: frame.width,
            height: frame.height,
            image_url,
            received_at,
            observed_fps,
        })
    }
}

#[component]
pub(super) fn FrameView(
    latest: Signal<Option<PresentedFrame>>,
    now: Instant,
    live: bool,
) -> Element {
    let guard = latest.read();
    let Some(frame) = guard.as_ref() else {
        return rsx! {};
    };
    let age_ms = now.saturating_duration_since(frame.received_at).as_millis();
    let observed = frame.observed_fps.map_or_else(
        || "waiting for second frame".to_owned(),
        |fps| format!("{fps:.1} FPS"),
    );

    rsx! {
        div {
            class: "space-y-3",
            p {
                class: HELP,
                "Observed: {observed}. {frame.width} × {frame.height}; image clock {frame.timestamp_us} µs. Received {age_ms} ms ago."
            }
            if !live {
                p { class: HELP, "Stopped / frozen image — this is not a live tracking view." }
            } else if age_ms > 2000 {
                p { role: "status", class: HELP, "Waiting for a new complete frame; displayed image is stale." }
            }
            // Lossless native-resolution pixels, scaled only for display.
            div {
                style: "position:relative;width:100%;background:#000;line-height:0;",
                img {
                    src: frame.image_url.as_str(),
                    alt: "Full-resolution grayscale infrared sensor image",
                    width: "{frame.width}",
                    height: "{frame.height}",
                    style: "display:block;width:100%;height:auto;image-rendering:pixelated;",
                }
            }
        }
    }
}
