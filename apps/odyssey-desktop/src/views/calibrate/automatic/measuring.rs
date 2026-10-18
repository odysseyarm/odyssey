//! Shown while a measurement runs, first the screen and then the markers.
//! Progress in words and a bar, and a Cancel button that stops everything.

use dioxus::prelude::*;

use super::hints::{progress, Progress};
use super::ui::{StepPage, SECONDARY_BUTTON};
use super::wizard::Wizard;
use crate::components::wizard::Spinner;

#[component]
pub fn MeasuringStep(
    wizard: Wizard,
    title: String,
    /// What is being measured, for the progress sentence: "the screen".
    what: String,
    hint: String,
) -> Element {
    // Until the camera thread reports its first step there is nothing to count.
    let shown = match *wizard.progress.read() {
        Some(step) => progress(step, &what),
        None => Progress { text: "Getting ready…".into(), fraction: 0.0 },
    };
    let percent = (shown.fraction * 100.0).round();

    rsx! {
        StepPage {
            title,
            subtitle: hint,
            footer: rsx! {
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.back_to_setup(),
                    "Cancel"
                }
                span {}
            },
            div {
                class: "flex flex-col items-center gap-6 rounded-xl bg-white px-6 py-14 shadow dark:bg-gray-700",
                Spinner { class: "size-14 text-blue-600".to_string() }
                div { class: "text-2xl font-semibold text-gray-900 dark:text-white", "{shown.text}" }
                div {
                    class: "h-3 w-full max-w-md overflow-hidden rounded-full bg-gray-200 dark:bg-gray-600",
                    role: "progressbar",
                    aria_valuemin: "0",
                    aria_valuemax: "100",
                    aria_valuenow: "{percent}",
                    div {
                        class: "h-full rounded-full bg-blue-600 transition-all duration-200",
                        style: "width: {percent}%;",
                    }
                }
            }
        }
    }
}
