use std::sync::Arc;
use std::time::Instant;

use gpui::prelude::FluentBuilder as _;
use gpui::{IntoElement, ParentElement as _, RenderImage, Styled as _, div, img};
use image::{Frame, RgbaImage};
use odyssey_protocol::image::ImageFrame;

use crate::theme::Palette;

/// Keep the converted pixels once instead of converting on every receive-age
/// tick.
pub(super) struct PresentedFrame {
    timestamp_us: u32,
    width: u16,
    height: u16,
    pub(super) image: Arc<RenderImage>,
    received_at: Instant,
    observed_fps: Option<f64>,
}

impl PresentedFrame {
    pub(super) fn new(
        frame: ImageFrame,
        received_at: Instant,
        observed_fps: Option<f64>,
    ) -> Result<Self, String> {
        // GPUI textures are BGRA; the sensor sends one gray byte per pixel.
        let bgra: Vec<u8> = frame
            .pixels
            .iter()
            .flat_map(|&gray| [gray, gray, gray, u8::MAX])
            .collect();
        let buffer = RgbaImage::from_raw(u32::from(frame.width), u32::from(frame.height), bgra)
            .ok_or_else(|| {
                format!(
                    "{} gray pixels do not fill a {} × {} image",
                    frame.pixels.len(),
                    frame.width,
                    frame.height
                )
            })?;
        Ok(Self {
            timestamp_us: frame.timestamp_us,
            width: frame.width,
            height: frame.height,
            image: Arc::new(RenderImage::new(vec![Frame::new(buffer)])),
            received_at,
            observed_fps,
        })
    }
}

pub(super) fn frame_view(
    frame: &PresentedFrame,
    now: Instant,
    live: bool,
    palette: Palette,
) -> impl IntoElement {
    let age_ms = now.saturating_duration_since(frame.received_at).as_millis();
    let observed = frame.observed_fps.map_or_else(
        || "waiting for second frame".to_owned(),
        |fps| format!("{fps:.1} FPS"),
    );
    let note = |text: &'static str| div().text_sm().text_color(palette.fg_muted).child(text);

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(div().text_sm().text_color(palette.fg_muted).child(format!(
            "Observed: {observed}. {} × {}; image clock {} µs. Received {age_ms} ms ago.",
            frame.width, frame.height, frame.timestamp_us
        )))
        .when(!live, |this| {
            this.child(note(
                "Stopped / frozen image — this is not a live tracking view.",
            ))
        })
        .when(live && age_ms > 2000, |this| {
            this.child(note(
                "Waiting for a new complete frame; displayed image is stale.",
            ))
        })
        // Native-resolution pixels, scaled only for display.
        .child(
            div().w_full().bg(gpui::black()).child(
                img(frame.image.clone())
                    .w_full()
                    .aspect_ratio(f32::from(frame.width) / f32::from(frame.height)),
            ),
        )
}

#[cfg(test)]
mod tests {
    use odyssey_protocol::Port;

    use super::*;

    fn frame(width: u16, height: u16, pixels: Vec<u8>) -> ImageFrame {
        ImageFrame {
            frame_id: 0,
            timestamp_us: 0,
            camera: Port::Nf,
            width,
            height,
            pixels,
            objects: Vec::new(),
            objects_timestamp_us: 0,
        }
    }

    #[test]
    fn gray_pixels_become_opaque_bgra() {
        let presented =
            PresentedFrame::new(frame(2, 1, vec![0, 200]), Instant::now(), None).expect("valid");
        assert_eq!(
            presented.image.as_bytes(0),
            Some([0, 0, 0, 255, 200, 200, 200, 255].as_slice())
        );
    }

    #[test]
    fn pixel_count_must_match_dimensions() {
        assert!(PresentedFrame::new(frame(2, 2, vec![0; 3]), Instant::now(), None).is_err());
    }
}
