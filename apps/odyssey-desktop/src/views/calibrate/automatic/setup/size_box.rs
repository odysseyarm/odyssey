//! The optional size box, in the unit of the field being asked for.

use dioxus::prelude::*;

use crate::views::calibrate::automatic::settings::{
    DIAGONAL_RANGE_IN, SizeError, SizeField, WIDTH_RANGE_CM,
};
use crate::views::calibrate::automatic::wizard::Wizard;

#[component]
pub(super) fn SizeBox(wizard: Wizard, label: String) -> Element {
    let field = wizard.field();
    // A half-typed number such as the "4" of 40 is out of range for a moment;
    // that is reported once the tech leaves the box, not while they type.
    let mut left_box = use_signal(|| false);
    let error = wizard
        .entered()
        .err()
        .filter(|e| *left_box.read() || matches!(e, SizeError::NotANumber))
        .map(SizeError::message);
    let (unit, placeholder, range) = match field {
        SizeField::Diagonal => (
            "inches",
            "Measure it for me",
            format!("{}–{}", DIAGONAL_RANGE_IN.start(), DIAGONAL_RANGE_IN.end()),
        ),
        SizeField::Width => (
            "cm",
            "Measure it for me",
            format!("{}–{}", WIDTH_RANGE_CM.start(), WIDTH_RANGE_CM.end()),
        ),
    };
    rsx! {
        div {
            class: "flex flex-col gap-1",
            label {
                r#for: "size-input",
                class: "text-base font-medium text-gray-900 dark:text-white",
                {label}
            }
            div {
                class: "flex items-center gap-2",
                input {
                    id: "size-input",
                    r#type: "text",
                    inputmode: "decimal",
                    autocomplete: "off",
                    placeholder,
                    title: "{range} {unit}",
                    class: "w-48 rounded-lg border border-gray-300 bg-white px-3 py-2 text-lg text-gray-900 focus:border-blue-500 focus:outline-none focus:ring-4 focus:ring-blue-200 dark:border-gray-500 dark:bg-gray-800 dark:text-white dark:focus:ring-blue-900",
                    value: "{wizard.size_text}",
                    oninput: move |e| {
                        left_box.set(false);
                        *wizard.size_text.write() = e.value();
                    },
                    onblur: move |_| left_box.set(true),
                }
                span { class: "text-lg text-gray-600 dark:text-gray-300", {unit} }
            }
            if let Some(error) = error {
                span { role: "alert", class: "text-sm text-red-700 dark:text-red-300", {error} }
            }
        }
    }
}
