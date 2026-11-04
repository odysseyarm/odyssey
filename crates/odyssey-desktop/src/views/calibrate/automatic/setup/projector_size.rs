//! The size question for a projector: optional, as a diagonal or a width.

use dioxus::prelude::*;

use super::size_box::SizeBox;
use crate::views::calibrate::automatic::settings::SizeField;
use crate::views::calibrate::automatic::wizard::Wizard;

const SIZE_FIELD_SELECTED: &str = "bg-blue-600 px-4 py-2 text-base font-medium text-white focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300";
const SIZE_FIELD_IDLE: &str = "bg-white px-4 py-2 text-base font-medium text-gray-800 hover:bg-gray-100 focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:bg-gray-800 dark:text-gray-100 dark:hover:bg-gray-700";

#[component]
pub(super) fn ProjectorSize(wizard: Wizard) -> Element {
    let field = wizard.field();
    rsx! {
        div {
            class: "flex flex-col gap-3",
            p {
                class: "text-lg font-medium text-gray-900 dark:text-white",
                "We'll measure the picture with the camera."
            }
            p {
                class: "text-base text-gray-600 dark:text-gray-300",
                "If you happen to know how big the picture is, type it in for a slightly more exact result. Otherwise leave it empty."
            }
            div {
                role: "radiogroup",
                aria_label: "Which size do you know?",
                class: "inline-flex w-fit overflow-hidden rounded-lg border border-gray-300 dark:border-gray-500",
                for (option, label) in [(SizeField::Diagonal, "Diagonal"), (SizeField::Width, "Width")] {
                    button {
                        key: "{label}",
                        r#type: "button",
                        role: "radio",
                        aria_checked: option == field,
                        class: if option == field { SIZE_FIELD_SELECTED } else { SIZE_FIELD_IDLE },
                        onclick: move |_| {
                            if *wizard.size_field.peek() != option {
                                wizard.size_text.write().clear();
                                *wizard.size_field.write() = option;
                            }
                        },
                        {label}
                    }
                }
            }
            SizeBox {
                wizard,
                label: match field {
                    SizeField::Diagonal => "Picture size (diagonal)",
                    SizeField::Width => "Picture width",
                },
            }
        }
    }
}
