//! The camera picture with the fitted screen outline and the marker positions.

use std::sync::Arc;

use gpui::{Div, RenderImage, Rgba};

use crate::components::wizard::{overlay_image, overlay_label};
use crate::theme::Palette;
use crate::views::calibrate::automatic::quality::Verdict;

/// The width the overlay's stroke sizes are specified for. Wider pictures
/// scale them up to stay equally visible.
const REFERENCE_WIDTH_PX: f32 = 640.0;

pub(super) fn outline_color(p: &Palette, verdict: Verdict) -> Rgba {
    match verdict {
        Verdict::Good => p.success,
        Verdict::Fair => p.warning,
        Verdict::Poor => p.danger,
    }
}

pub(super) fn result_overlay(
    p: &Palette,
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
    screen_quad: [[f32; 2]; 4],
    markers: [[f32; 2]; 6],
    outline: Rgba,
) -> Div {
    let unit = width as f32 / REFERENCE_WIDTH_PX;
    let info = p.info;
    let labels = markers
        .iter()
        .enumerate()
        .map(|(i, marker)| {
            overlay_label(
                marker[0] + 12.0 * unit,
                marker[1] - 10.0 * unit,
                width,
                height,
                info,
                (i + 1).to_string(),
            )
        })
        .collect();
    overlay_image(
        Some(image),
        width,
        height,
        move |canvas, window| {
            canvas.stroke_path(window, &screen_quad, true, 3.0 * unit, outline);
            for marker in &markers {
                canvas.stroke_circle(window, *marker, 9.0 * unit, 2.5 * unit, info);
            }
        },
        labels,
    )
}
