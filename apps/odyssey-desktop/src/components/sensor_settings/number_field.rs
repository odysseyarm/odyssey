use dioxus::prelude::*;

pub(super) const INPUT_CLASS: &str = "bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg \
    focus:ring-blue-500 focus:border-blue-500 block w-full p-1.5 \
    dark:bg-gray-700 dark:border-gray-600 dark:text-white \
    dark:focus:ring-blue-500 dark:focus:border-blue-500";

pub(super) const LABEL_CLASS: &str = "text-sm text-gray-700 dark:text-gray-300 whitespace-nowrap";

pub(super) const INVALID_INPUT_CLASS: &str = "bg-gray-50 border border-red-500 text-gray-900 text-sm rounded-lg \
    focus:ring-red-500 focus:border-red-500 block w-full p-1.5 \
    dark:bg-gray-700 dark:border-red-500 dark:text-white \
    dark:focus:ring-red-500 dark:focus:border-red-500";

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
        label { class: LABEL_CLASS, "{label}" }
        div {
            class: "space-y-0.5",
            div {
                class: "flex items-center gap-2",
                input {
                    class: if invalid { INVALID_INPUT_CLASS } else { INPUT_CLASS },
                    r#type: "text",
                    inputmode: "numeric",
                    value: "{value}",
                    oninput: move |e| oninput.call(e.value()),
                }
                if let Some(hint) = hint {
                    span {
                        class: "text-xs text-gray-500 dark:text-gray-400 whitespace-nowrap",
                        "{hint}"
                    }
                }
            }
            if let Some(issue) = issue {
                p { class: "text-xs text-red-600 dark:text-red-400", "{issue}" }
            }
        }
    }
}
