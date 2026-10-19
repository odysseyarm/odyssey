//! Step 4: the screen is black and the tech turns on the tracker markers. The
//! infrared view shows what the camera sees. Measuring starts when the tech presses
//! Measure, never by itself.

use std::time::Duration;

use dioxus::prelude::*;

use super::hints::{MARKER_RULES, marker_hint};
use super::settings::{IR_EXPOSURE_RANGE_US, IR_EXPOSURE_STEP_US};
use super::ui::{
    Details, Fact, PRIMARY_BUTTON, PRIMARY_ID, SECONDARY_BUTTON, StepPage, focus_primary,
};
use super::wizard::Wizard;
use crate::components::wizard::{Icon, OverlayImage, Slider, StatusBanner, Tone};

/// How long the marker brightness slider rests before the camera follows it.
const EXPOSURE_DEBOUNCE: Duration = Duration::from_millis(200);

#[component]
pub fn MarkersStep(wizard: Wizard) -> Element {
    // Measuring starts only when the user presses Measure, which is live once all six
    // markers are found.
    let all_found = use_memo(move || {
        wizard
            .markers
            .read()
            .as_ref()
            .is_some_and(|f| f.matched.is_some())
    });
    use_effect(move || {
        if all_found() {
            focus_primary();
        }
    });
    let ir = *wizard.ir_exposure.read();
    let mut generation = use_signal(|| 0u64);

    rsx! {
        StepPage {
            title: "Turn on the tracker markers",
            subtitle: "The screen is dark now. Switch on all six markers and keep the camera where it is. Press Measure when you're ready.",
            footer: rsx! {
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.back_to_setup(),
                    "Cancel"
                }
                button {
                    id: PRIMARY_ID,
                    class: PRIMARY_BUTTON,
                    r#type: "button",
                    disabled: !all_found(),
                    onclick: move |_| wizard.measure_markers(),
                    "Measure"
                }
            },

            MarkerBanner { wizard }
            MarkerPreviewView { wizard }

            div {
                class: "rounded-xl bg-white p-4 shadow dark:bg-gray-700",
                Slider {
                    id: "ir-exposure",
                    label: "Marker brightness",
                    value: ir,
                    min: *IR_EXPOSURE_RANGE_US.start(),
                    max: *IR_EXPOSURE_RANGE_US.end(),
                    step: IR_EXPOSURE_STEP_US,
                    value_text: format!("{ir} µs"),
                    help: "Only the six markers should show up as bright dots. Lower it if other lights appear, raise it if a marker is missing.",
                    oninput: move |us| {
                        wizard.set_ir_exposure(us);
                        let this = *generation.peek() + 1;
                        generation.set(this);
                        spawn(async move {
                            tokio::time::sleep(EXPOSURE_DEBOUNCE).await;
                            // A later movement of the slider supersedes this one.
                            if *generation.peek() == this {
                                wizard.apply_ir_exposure();
                            }
                        });
                    },
                }
            }

            MarkerDetails { wizard }
            MarkerRules {}
        }
    }
}

#[component]
fn MarkerBanner(wizard: Wizard) -> Element {
    let markers = wizard.markers.read();
    let Some(frame) = markers.as_ref() else {
        return rsx! {
            StatusBanner {
                tone: Tone::Attention,
                icon: Icon::Camera,
                title: "Switching the camera to marker view…",
                detail: "This takes a moment.",
            }
        };
    };
    let message = marker_hint(frame.hint);
    rsx! {
        StatusBanner { tone: message.tone, icon: message.icon, title: message.title, detail: message.detail }
    }
}

#[component]
fn MarkerPreviewView(wizard: Wizard) -> Element {
    let markers = wizard.markers.read();
    let Some(frame) = markers.as_ref() else {
        return rsx! {
            div {
                class: "flex aspect-video w-full items-center justify-center rounded-xl bg-gray-900 text-gray-400",
                "Waiting for the camera…"
            }
        };
    };
    let unit = f64::from(frame.width) / 640.0;
    let radius = 11.0 * unit;
    let line = 2.5 * unit;
    rsx! {
        OverlayImage {
            src: frame.image.clone(),
            width: frame.width,
            height: frame.height,
            alt: "Infrared camera picture",
            for (i, blob) in frame.blobs.iter().enumerate() {
                {
                    // A blob matched to one of the six markers is green and numbered.
                    let marker = frame.matched.and_then(|m| m.iter().position(|&b| b == i));
                    let color = if marker.is_some() { "#22c55e" } else { "#f59e0b" };
                    rsx! {
                        circle {
                            key: "{i}",
                            cx: "{blob[0]}",
                            cy: "{blob[1]}",
                            r: "{radius}",
                            fill: "none",
                            stroke: "{color}",
                            stroke_width: "{line}",
                        }
                        if let Some(marker) = marker {
                            text {
                                x: "{f64::from(blob[0]) + radius * 1.3}",
                                y: "{f64::from(blob[1]) - radius * 1.1}",
                                fill: "{color}",
                                font_size: "{radius * 1.7}",
                                font_weight: "700",
                                "{marker + 1}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MarkerDetails(wizard: Wizard) -> Element {
    let markers = wizard.markers.read();
    rsx! {
        Details {
            if let Some(frame) = markers.as_ref() {
                Fact { label: "Bright spots seen", value: frame.blobs.len().to_string() }
                Fact {
                    label: "Matched to the marker layout",
                    value: if frame.matched.is_some() { "6 of 6".to_string() } else { "not all six".to_string() },
                }
                Fact { label: "Picture size", value: format!("{} × {}", frame.width, frame.height) }
            }
            Fact { label: "Infrared exposure", value: format!("{} µs", *wizard.ir_exposure.read()) }
        }
    }
}

/// The rules for where markers can go, collapsed until asked for.
#[component]
fn MarkerRules() -> Element {
    rsx! {
        Details { summary: "Where the markers can go",
            for (title, text) in MARKER_RULES {
                div {
                    key: "{title}",
                    class: "flex flex-col gap-0.5",
                    span { class: "font-medium", "{title}" }
                    span { class: "text-gray-600 dark:text-gray-300", "{text}" }
                }
            }
        }
    }
}
