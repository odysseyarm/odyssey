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
                class: "text-base font-medium text-gray-500 dark:text-gray-400",
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
                            "border-blue-600 bg-blue-600 text-white"
                        } else {
                            "border-gray-300 bg-white text-gray-800 hover:border-blue-400 dark:border-gray-500 dark:bg-gray-800 dark:text-gray-100"
                        };
                        let status_class = if selected { "opacity-90" } else { "text-gray-500 dark:text-gray-400" };
                        rsx! {
                            button {
                                key: "{id}",
                                r#type: "button",
                                role: "radio",
                                aria_checked: selected,
                                class: "flex flex-col items-center rounded-lg border-2 px-2 py-2 focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:focus-visible:ring-blue-800 {class}",
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
                    class: "text-sm text-amber-700 dark:text-amber-300",
                    "{screen_label(chosen)} already has a calibration. You'll be asked before it is replaced."
                }
            }
        }
    }
}
