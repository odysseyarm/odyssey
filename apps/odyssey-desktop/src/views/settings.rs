use dioxus::prelude::*;

#[component]
pub fn Settings() -> Element {
    rsx! {
        div { class: "min-h-screen bg-gray-50 dark:bg-gray-800 p-6",
            div { class: "w-full max-w-xl mx-auto flex flex-col gap-4",
                h1 { class: "text-2xl font-bold text-gray-900 dark:text-white", "Settings" }

                div { class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow space-y-4",
                    h2 { class: "text-lg font-semibold text-gray-900 dark:text-white", "About" }

                    p { class: "text-sm text-gray-600 dark:text-gray-300",
                        {format!("Odyssey Desktop v{}", env!("CARGO_PKG_VERSION"))}
                    }
                }
            }
        }
    }
}
