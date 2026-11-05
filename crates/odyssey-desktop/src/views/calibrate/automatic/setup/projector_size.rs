//! The size question for a projector: optional, as a diagonal or a width.

use dioxus::prelude::*;

use super::size_box::SizeBox;
use crate::views::calibrate::automatic::settings::SizeField;
use crate::views::calibrate::automatic::wizard::Wizard;

const SIZE_FIELD_SELECTED: &str = "bg-accent px-4 py-2 text-base font-medium text-on-accent";
const SIZE_FIELD_IDLE: &str =
    "bg-surface px-4 py-2 text-base font-medium text-fg hover:bg-surface-hover";

#[component]
pub(super) fn ProjectorSize(wizard: Wizard) -> Element {
    let field = wizard.field();
    rsx! {
        div {
            class: "flex flex-col gap-3",
            p {
                class: "text-lg font-medium text-fg-strong",
                "We'll measure the picture with the camera."
            }
            p {
                class: "text-base text-fg-muted",
                "If you happen to know how big the picture is, type it in for a slightly more exact result. Otherwise leave it empty."
            }
            div {
                role: "radiogroup",
                aria_label: "Which size do you know?",
                class: "inline-flex w-fit overflow-hidden rounded-lg border border-line",
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
