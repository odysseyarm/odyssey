//! Shown after a successful save.

use dioxus::prelude::*;

use crate::components::wizard::{Icon, IconView};
use crate::views::calibrate::automatic::screens::screen_label;
use crate::views::calibrate::automatic::ui::{
    Details, Fact, PRIMARY_BUTTON, PRIMARY_ID, SECONDARY_BUTTON, StepPage,
};
use crate::views::calibrate::automatic::wizard::Wizard;

#[component]
pub fn SavedStep(wizard: Wizard) -> Element {
    let Some(saved) = wizard.saved_to.read().clone() else {
        return rsx! {};
    };
    rsx! {
        StepPage {
            title: "Calibration saved",
            subtitle: "{screen_label(saved.screen_id)} is ready to use.",
            footer: rsx! {
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.calibrate_another(),
                    "Calibrate another screen"
                }
                button {
                    id: PRIMARY_ID,
                    class: PRIMARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.exit(),
                    "Done"
                }
            },

            div {
                class: "alert alert-success flex flex-col items-center gap-3 px-6 py-8 text-center",
                IconView { icon: Icon::Check, class: "size-16 text-success-ink".to_string() }
                div { class: "text-2xl font-semibold", "All done" }
                div { class: "text-base", "The new calibration is active as soon as the hub picks it up, which takes a second or two." }
            }

            section {
                class: "card flex items-center justify-between gap-4 px-5 py-4",
                p {
                    class: "text-base text-fg",
                    span { class: "font-semibold text-fg-strong", "Recommended: " }
                    "re-zero your devices so aiming matches the new calibration."
                }
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.go_home(),
                    "Go to Zero"
                }
            }

            Details {
                Fact { label: "Saved to", value: saved.path.clone() }
            }
        }
    }
}
