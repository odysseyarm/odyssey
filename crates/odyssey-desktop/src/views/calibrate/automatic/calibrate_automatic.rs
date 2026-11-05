//! The route: the wizard's frame, its header and the step that matches its stage.

use dioxus::prelude::*;

use super::wizard::{Stage, Wizard, use_wizard};
use super::{markers, measuring, position, problem, review, setup};
use crate::components::Theme;
use crate::components::wizard::Stepper;

#[component]
pub fn CalibrateAutomatic() -> Element {
    let wizard = use_wizard();
    let stage = *wizard.stage.read();
    let step = *wizard.step.read();

    rsx! {
        Theme {}

        div {
            class: "min-h-screen bg-canvas",
            tabindex: 0,
            onkeydown: move |e| {
                // Escape steps back, like the Back button on every step.
                if e.key() == Key::Escape && *wizard.stage.peek() != Stage::Saved {
                    wizard.back_to_setup();
                }
            },

            div {
                class: "mx-auto flex w-full max-w-3xl flex-col gap-6 px-6 py-6",

                WizardHeader { wizard }

                Stepper { steps: Stage::STEPS.to_vec(), current: step }

                main {
                    StageView { wizard, stage }
                }
            }
        }
    }
}

#[component]
fn WizardHeader(wizard: Wizard) -> Element {
    rsx! {
        header {
            class: "flex items-center justify-between gap-4",
            div {
                class: "flex items-center gap-3",
                span { class: "eyebrow", "Calibrate screen · Automatic" }
                if wizard.demo {
                    span {
                        class: "badge badge-accent",
                        title: "A simulated camera and screen are used. Nothing real is measured.",
                        "Demo · simulated camera"
                    }
                }
            }
            button {
                class: "btn btn-ghost btn-sm",
                r#type: "button",
                onclick: move |_| wizard.exit(),
                "Exit"
            }
        }
    }
}

#[component]
fn StageView(wizard: Wizard, stage: Stage) -> Element {
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
