use dioxus::prelude::*;

use crate::components::update::{UpdateChannelPicker, UpdateState};

#[component]
pub fn Settings() -> Element {
    let mut state = use_context::<UpdateState>();

    let status = if (state.checking)() {
        "Checking for updates…".to_string()
    } else if let Some(err) = (state.error)() {
        format!("Update check failed: {err}")
    } else if (state.available)() {
        "Update available — install it from the banner above.".to_string()
    } else {
        "Up to date.".to_string()
    };

    rsx! {
        div { class: "min-h-screen bg-gray-50 dark:bg-gray-800 p-6",
            div { class: "w-full max-w-xl mx-auto flex flex-col gap-4",
                h1 { class: "text-2xl font-bold text-gray-900 dark:text-white", "Settings" }

                div { class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow space-y-4",
                    h2 { class: "text-lg font-semibold text-gray-900 dark:text-white", "Updates" }

                    p { class: "text-sm text-gray-600 dark:text-gray-300",
                        {format!("Odyssey Desktop v{}", env!("CARGO_PKG_VERSION"))}
                    }

                    div { class: "max-w-xs",
                        UpdateChannelPicker {}
                    }

                    div { class: "flex items-center gap-3",
                        button {
                            class: "bg-gray-200 dark:bg-gray-600 text-gray-900 dark:text-white rounded px-3 py-1 text-sm font-medium hover:bg-gray-300 dark:hover:bg-gray-500 disabled:opacity-60",
                            disabled: (state.checking)(),
                            onclick: move |_| state.request_check(),
                            "Check for updates"
                        }
                        span { class: "text-sm text-gray-600 dark:text-gray-300", "{status}" }
                    }
                }
            }
        }
    }
}
