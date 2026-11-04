//! The camera picture with the fitted screen outline and the marker positions.

use dioxus::prelude::*;

use crate::components::wizard::OverlayImage;
use crate::views::calibrate::automatic::quality::Verdict;

/// The width the overlay's stroke sizes are specified for; wider pictures
/// scale them up to stay equally visible.
const REFERENCE_WIDTH_PX: f64 = 640.0;
const MARKER_COLOR: &str = "#38bdf8";

pub(super) fn outline_color(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Good => "#22c55e",
        Verdict::Fair => "#f59e0b",
        Verdict::Poor => "#ef4444",
    }
}

/// The corners as an SVG `points` list.
pub(super) fn polygon_points(corners: &[[f32; 2]]) -> String {
    corners
        .iter()
        .map(|corner| format!("{:.1},{:.1}", corner[0], corner[1]))
        .collect::<Vec<_>>()
        .join(" ")
}

#[component]
pub(super) fn ResultOverlay(
    image: String,
    width: u32,
    height: u32,
    screen_quad: [[f32; 2]; 4],
    markers: [[f32; 2]; 6],
    outline: &'static str,
) -> Element {
    let points = polygon_points(&screen_quad);
    let unit = f64::from(width) / REFERENCE_WIDTH_PX;
    rsx! {
        OverlayImage {
            src: image,
            width,
            height,
            alt: "The screen as the camera saw it, with the measured outline",
            polygon {
                points,
                fill: "none",
                stroke: outline,
                stroke_width: "{3.0 * unit}",
                stroke_linejoin: "round",
            }
            for (i, marker) in markers.iter().enumerate() {
                circle {
                    key: "{i}",
                    cx: "{marker[0]}",
                    cy: "{marker[1]}",
                    r: "{9.0 * unit}",
                    fill: "none",
                    stroke: MARKER_COLOR,
                    stroke_width: "{2.5 * unit}",
                }
                text {
                    x: "{f64::from(marker[0]) + 12.0 * unit}",
                    y: "{f64::from(marker[1]) - 10.0 * unit}",
                    fill: MARKER_COLOR,
                    font_size: "{16.0 * unit}",
                    font_weight: "700",
                    "{i + 1}"
                }
            }
        }
    }
}
