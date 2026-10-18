//! Step 2: aim the camera. Live picture with detection overlays, one plain
//! sentence about what to change, and a brightness slider.

use std::time::Duration;

use dioxus::prelude::*;
use odyssey_autocalib::session::Hint;

use super::hints::{OVERSCAN_HELP, screen_hint};
use super::settings::{COLOR_EXPOSURE_RANGE_US, COLOR_EXPOSURE_STEP_US};
use super::ui::{
    Details, Fact, PRIMARY_BUTTON, PRIMARY_ID, SECONDARY_BUTTON, StepPage, focus_primary,
};
use super::wizard::Wizard;
use super::worker::PreviewFrame;
use crate::components::wizard::{Icon, OverlayImage, Slider, StatusBanner, Tone};

/// How long the brightness slider rests before the stream restarts with it.
const EXPOSURE_DEBOUNCE: Duration = Duration::from_millis(450);

/// Brightness slider position as a percentage of its range.
fn exposure_percent(us: u32) -> u32 {
    let (lo, hi) = (
        *COLOR_EXPOSURE_RANGE_US.start(),
        *COLOR_EXPOSURE_RANGE_US.end(),
    );
    (us.clamp(lo, hi) - lo) * 100 / (hi - lo)
}

#[component]
pub fn PositionStep(wizard: Wizard) -> Element {
    let same_monitor = *wizard.same_monitor.read();
    let ready = use_memo(move || wizard.position_ready());
    let has_frame = use_memo(move || wizard.preview.read().is_some());
    let mut generation = use_signal(|| 0u64);

    // The primary button becomes available while the tech holds the camera, so
    // Enter is the natural next move.
    use_effect(move || {
        let ready_now = if same_monitor { has_frame() } else { ready() };
        if ready_now {
            focus_primary();
        }
    });

    let exposure = *wizard.color_exposure.read();
    let restarting = *wizard.restarting.read();

    rsx! {
        StepPage {
            title: "Point the camera at the screen",
            subtitle: if same_monitor {
                "Aim the camera at the screen, then press Start. This window steps aside while the screen is measured."
            } else {
                "Hold the camera in front of the screen so the whole dot pattern is in view."
            },
            footer: rsx! {
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.back_to_setup(),
                    "Back"
                }
                if same_monitor {
                    button {
                        id: PRIMARY_ID,
                        class: PRIMARY_BUTTON,
                        r#type: "button",
                        disabled: !has_frame(),
                        onclick: move |_| wizard.start_countdown(),
                        "Start"
                    }
                } else {
                    button {
                        id: PRIMARY_ID,
                        class: PRIMARY_BUTTON,
                        r#type: "button",
                        disabled: !ready(),
                        onclick: move |_| wizard.measure_screen(),
                        "Next"
                    }
                }
            },

            Banner { wizard }

            LivePreview { wizard }

            div {
                class: "rounded-xl bg-white p-4 shadow dark:bg-gray-700",
                Slider {
                    id: "color-exposure",
                    label: "Brightness",
                    value: exposure,
                    min: *COLOR_EXPOSURE_RANGE_US.start(),
                    max: *COLOR_EXPOSURE_RANGE_US.end(),
                    step: COLOR_EXPOSURE_STEP_US,
                    value_text: if restarting { "Applying…".to_string() } else { format!("{}%", exposure_percent(exposure)) },
                    help: "If the dots look washed out or too faint, adjust this until they are crisp white dots on black.",
                    oninput: move |us| {
                        wizard.set_color_exposure(us);
                        let this = *generation.peek() + 1;
                        generation.set(this);
                        spawn(async move {
                            tokio::time::sleep(EXPOSURE_DEBOUNCE).await;
                            // A later movement of the slider supersedes this one.
                            if *generation.peek() == this {
                                wizard.restart_camera();
                            }
                        });
                    },
                }
            }

            PositionDetails { wizard }
        }
    }
}

#[component]
fn Banner(wizard: Wizard) -> Element {
    let same_monitor = *wizard.same_monitor.read();
    let preview = wizard.preview.read();

    if same_monitor {
        return rsx! {
            StatusBanner {
                tone: if preview.is_some() { Tone::Good } else { Tone::Attention },
                icon: if preview.is_some() { Icon::Camera } else { Icon::Viewfinder },
                title: if preview.is_some() { "Aim the camera at the screen" } else { "Starting the camera…" },
                detail: "The dot pattern appears after a short countdown, so there is nothing to line up yet. Make sure the whole screen is in the picture.",
            }
        };
    }

    let Some(frame) = preview.as_ref() else {
        return rsx! {
            StatusBanner {
                tone: Tone::Attention,
                icon: Icon::Camera,
                title: "Starting the camera…",
                detail: "This takes a few seconds.",
            }
        };
    };

    let message = if frame.hint == Hint::Ready && !wizard.position_ready() {
        // Ready for a moment is not ready yet; asking for a still hand avoids a
        // measurement started on a passing glimpse.
        let mut hold = screen_hint(Hint::ScreenNotFound);
        hold.icon = Icon::Camera;
        hold.title = "Almost there — hold it steady".into();
        hold.detail = "Keep the camera still for a moment.".into();
        hold
    } else {
        screen_hint(frame.hint)
    };
    rsx! {
        StatusBanner { tone: message.tone, icon: message.icon, title: message.title, detail: message.detail }
    }
}

#[component]
fn LivePreview(wizard: Wizard) -> Element {
    let preview = wizard.preview.read();
    let Some(frame) = preview.as_ref() else {
        return rsx! {
            div {
                class: "flex aspect-video w-full items-center justify-center rounded-xl bg-gray-900 text-gray-400",
                "Waiting for the camera…"
            }
        };
    };
    let ready = frame.hint == Hint::Ready;
    let color = if ready { "#22c55e" } else { "#f59e0b" };
    let unit = f64::from(frame.width) / 640.0;
    let dot_radius = 6.0 * unit;
    let line = 3.0 * unit;
    let quad = frame.quad.map(|q| {
        q.iter()
            .map(|p| format!("{:.1},{:.1}", p[0], p[1]))
            .collect::<Vec<_>>()
            .join(" ")
    });
    // Edges the display cuts off, drawn over the outline in red so the tech
    // sees which sides to look at. The quad runs top left, top right, bottom
    // right, bottom left.
    let cropped_edges: Vec<[[f32; 2]; 2]> = match (frame.quad, frame.cropped) {
        (Some(q), Some(sides)) => [
            (sides.top, 0),
            (sides.right, 1),
            (sides.bottom, 2),
            (sides.left, 3),
        ]
        .into_iter()
        .filter_map(|(on, i)| on.then(|| [q[i], q[(i + 1) % 4]]))
        .collect(),
        _ => Vec::new(),
    };
    rsx! {
        OverlayImage {
            src: frame.image.clone(),
            width: frame.width,
            height: frame.height,
            alt: "Live camera picture",
            if let Some(points) = quad {
                polygon {
                    points: "{points}",
                    fill: "none",
                    stroke: "{color}",
                    stroke_width: "{line}",
                    stroke_linejoin: "round",
                }
            }
            for (i, edge) in cropped_edges.iter().enumerate() {
                line {
                    key: "cropped-{i}",
                    x1: "{edge[0][0]}",
                    y1: "{edge[0][1]}",
                    x2: "{edge[1][0]}",
                    y2: "{edge[1][1]}",
                    stroke: "#ef4444",
                    stroke_width: "{line * 2.2}",
                    stroke_linecap: "round",
                }
            }
            for (i, dot) in frame.dots.iter().enumerate() {
                circle {
                    key: "{i}",
                    cx: "{dot[0]}",
                    cy: "{dot[1]}",
                    r: "{dot_radius}",
                    fill: "none",
                    stroke: "{color}",
                    stroke_width: "{line * 0.7}",
                }
            }
        }
    }
}

fn depth_text(frame: &PreviewFrame) -> (String, String) {
    (
        frame.distance_m.map_or("—".into(), |d| format!("{d:.2} m")),
        frame
            .depth_coverage
            .map_or("—".into(), |c| format!("{:.0} %", c * 100.0)),
    )
}

#[component]
fn PositionDetails(wizard: Wizard) -> Element {
    let preview = wizard.preview.read();
    let exposure = *wizard.color_exposure.read();
    let camera = wizard.camera().map(|c| c.name).unwrap_or_default();
    rsx! {
        Details {
            Fact { label: "Camera", value: camera }
            if let Some(frame) = preview.as_ref() {
                {
                    let (distance, coverage) = depth_text(frame);
                    rsx! {
                        Fact { label: "Distance to screen", value: distance }
                        Fact { label: "Depth coverage on screen", value: coverage }
                        Fact { label: "Dots found", value: format!("{} of {}", frame.dots_matched, frame.dots_expected) }
                        Fact {
                            label: "Camera tilt",
                            value: frame.tilt_deg.map_or("—".into(), |(pitch, roll)| format!("pitch {pitch:.1}°, roll {roll:.1}°")),
                        }
                        Fact { label: "Picture size", value: format!("{} × {}", frame.width, frame.height) }
                    }
                }
            }
            Fact { label: "Color exposure", value: format!("{exposure} µs") }
            if preview.as_ref().is_some_and(|f| f.hint == Hint::EdgesCropped) {
                div {
                    class: "mt-1 flex flex-col gap-2 border-t border-gray-200 pt-3 dark:border-gray-600",
                    div { class: "font-medium text-gray-900 dark:text-white", "Where to turn off overscan" }
                    p { class: "text-gray-600 dark:text-gray-300", "Menu names differ between models and years. These are the usual places:" }
                    for (brand, steps) in OVERSCAN_HELP {
                        div {
                            key: "{brand}",
                            span { class: "font-semibold", "{brand}: " }
                            "{steps}"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_percent_spans_the_slider() {
        assert_eq!(exposure_percent(*COLOR_EXPOSURE_RANGE_US.start()), 0);
        assert_eq!(exposure_percent(*COLOR_EXPOSURE_RANGE_US.end()), 100);
        assert_eq!(exposure_percent(0), 0);
        assert_eq!(exposure_percent(u32::MAX), 100);
    }
}
