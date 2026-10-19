//! Automatic calibration: a depth camera measures the screen and the tracker
//! markers while this app shows a dot pattern, and the result is saved as a
//! screen calibration.
//!
//! [`CalibrateAutomatic`] is the route. It owns a [`wizard::Wizard`] and draws
//! the step that matches its stage; see `wizard.rs` for the flow.

mod attempt_bundle;
mod attempt_button;
mod camera_support;
pub mod hints;
mod illustrations;
pub mod kind;
mod markers;
mod measuring;
pub mod pattern_window;
mod position;
mod problem;
pub mod quality;
pub mod reference;
mod reference_helper;
mod review;
mod scale_board_info;
pub mod screens;
mod sdk_download;
pub mod settings;
mod setup;
mod ui;
pub mod wizard;
pub mod worker;

use dioxus::prelude::*;
use wizard::{Stage, use_wizard};

use crate::components::wizard::Stepper;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
pub fn CalibrateAutomatic() -> Element {
    let wizard = use_wizard();
    let stage = *wizard.stage.read();
    let step = *wizard.step.read();

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }

        div {
            class: "min-h-screen bg-gray-50 dark:bg-gray-800",
            tabindex: 0,
            onkeydown: move |e| {
                // Escape steps back, like the Back button on every step.
                if e.key() == Key::Escape && *wizard.stage.peek() != Stage::Saved {
                    wizard.back_to_setup();
                }
            },

            div {
                class: "mx-auto flex w-full max-w-3xl flex-col gap-6 px-6 py-6",

                header {
                    class: "flex items-center justify-between gap-4",
                    div {
                        class: "flex items-center gap-3",
                        span { class: "text-sm font-semibold uppercase tracking-wide text-gray-500 dark:text-gray-400", "Calibrate screen · Automatic" }
                        if wizard.demo {
                            span {
                                class: "rounded-full bg-purple-100 px-3 py-1 text-xs font-semibold text-purple-800 dark:bg-purple-900 dark:text-purple-100",
                                title: "A simulated camera and screen are used. Nothing real is measured.",
                                "Demo · simulated camera"
                            }
                        }
                    }
                    button {
                        class: "rounded-lg px-3 py-1.5 text-sm font-medium text-gray-600 hover:bg-gray-200 focus:outline-none focus-visible:ring-4 focus-visible:ring-gray-300 dark:text-gray-300 dark:hover:bg-gray-700",
                        r#type: "button",
                        onclick: move |_| wizard.exit(),
                        "Exit"
                    }
                }

                Stepper { steps: Stage::STEPS.to_vec(), current: step }

                main {
                    match stage {
                        Stage::Setup => rsx! { setup::SetupStep { wizard } },
                        Stage::Position => rsx! { position::PositionStep { wizard } },
                        Stage::Measuring => rsx! {
                            measuring::MeasuringStep {
                                wizard,
                                title: "Measuring the screen",
                                what: "the screen",
                                hint: "Keep the camera still. This takes a few seconds.",
                            }
                        },
                        Stage::Markers => rsx! { markers::MarkersStep { wizard } },
                        Stage::MeasuringMarkers => rsx! {
                            measuring::MeasuringStep {
                                wizard,
                                title: "Measuring the markers",
                                what: "the markers",
                                hint: "Keep the camera still and the markers on.",
                            }
                        },
                        Stage::Review => rsx! { review::ReviewStep { wizard } },
                        Stage::Saved => rsx! { review::SavedStep { wizard } },
                        Stage::Problem => rsx! { problem::ProblemStep { wizard } },
                    }
                }
            }
        }
    }
}
