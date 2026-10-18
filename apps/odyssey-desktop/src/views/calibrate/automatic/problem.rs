//! A step failed. Says what happened in plain words and offers the one sensible
//! retry; the raw error stays under Details for support.

use dioxus::prelude::*;

use super::hints::{failure_message, retry_label};
use super::ui::{Details, PRIMARY_BUTTON, PRIMARY_ID, SECONDARY_BUTTON, StepPage};
use super::wizard::Wizard;
use crate::components::wizard::StatusBanner;

#[component]
pub fn ProblemStep(wizard: Wizard) -> Element {
    let Some(failure) = wizard.failure.read().clone() else {
        return rsx! {};
    };
    let message = failure_message(failure.kind, failure.size_origin);
    rsx! {
        StepPage {
            title: "That didn't work",
            footer: rsx! {
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.back_to_setup(),
                    "Back to setup"
                }
                button {
                    id: PRIMARY_ID,
                    class: PRIMARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.retry(),
                    "{retry_label(failure.kind)}"
                }
            },
            StatusBanner { tone: message.tone, icon: message.icon, title: message.title, detail: message.detail }
            Details {
                code { class: "break-all text-sm", "{failure.details}" }
            }
        }
    }
}
