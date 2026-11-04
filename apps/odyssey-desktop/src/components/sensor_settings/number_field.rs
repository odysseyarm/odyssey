use dioxus::prelude::*;

use super::styles::{FIELD_CLASS, INVALID_FIELD_CLASS, LABEL_CLASS};

/// A labelled numeric field with an optional unit hint and validation message.
///
/// `hint` carries the converted engineering value (ms, fps) so the raw register
/// numbers stop being the only thing on screen.
#[component]
pub(super) fn NumberField(
    label: String,
    value: String,
    hint: Option<String>,
    issue: Option<String>,
    oninput: EventHandler<String>,
) -> Element {
    let invalid = issue.is_some();
    rsx! {
        label { class: LABEL_CLASS, {label} }
        div {
            class: "space-y-0.5",
            div {
                class: "flex items-center gap-2",
                input {
                    class: if invalid { INVALID_FIELD_CLASS } else { FIELD_CLASS },
                    r#type: "text",
                    inputmode: "numeric",
                    value,
                    oninput: move |e| oninput.call(e.value()),
                }
                if let Some(hint) = hint {
                    span {
                        class: "text-xs text-gray-500 dark:text-gray-400 whitespace-nowrap",
                        {hint}
                    }
                }
            }
            if let Some(issue) = issue {
                p { class: "text-xs text-red-600 dark:text-red-400", {issue} }
            }
        }
    }
}
