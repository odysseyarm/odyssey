//! The slot a finished calibration is saved under.

use dioxus::prelude::*;
use odyssey_common::MAX_SCREEN_ID;

use crate::views::calibrate::automatic::screens::screen_label;
use crate::views::calibrate::automatic::wizard::Wizard;

#[component]
pub(super) fn ScreenNumberPicker(wizard: Wizard) -> Element {
    let chosen = wizard.screen_id();
    let saved = *wizard.saved.read();
    rsx! {
        div {
            class: "flex flex-col gap-2",
            span {
                id: "screen-number-label",
                class: "text-base font-medium text-fg-muted",
                "Save the result as:"
            }
            div {
                role: "radiogroup",
                aria_labelledby: "screen-number-label",
                class: "grid grid-cols-3 gap-2 sm:grid-cols-6",
                for id in 0..=MAX_SCREEN_ID {
                    {
                        let selected = id == chosen;
                        let taken = saved[usize::from(id)];
                        let class = if selected {
                            "border-accent bg-accent text-on-accent"
                        } else {
                            "border-line bg-surface text-fg hover:border-accent-ink"
                        };
                        let status_class = if selected { "opacity-90" } else { "text-fg-muted" };
                        rsx! {
                            button {
                                key: "{id}",
                                r#type: "button",
                                role: "radio",
                                aria_checked: selected,
                                class: "flex flex-col items-center rounded-lg border-2 px-2 py-2 {class}",
                                onclick: move |_| *wizard.screen_choice.write() = Some(id),
                                span { class: "text-base font-semibold", "{screen_label(id)}" }
                                span {
                                    class: "text-xs {status_class}",
                                    if taken { "Calibrated" } else { "Empty" }
                                }
                            }
                        }
                    }
                }
            }
            if saved[usize::from(chosen)] {
                span {
                    class: "text-sm text-warning-ink",
                    "{screen_label(chosen)} already has a calibration. You'll be asked before it is replaced."
                }
            }
        }
    }
}
