//! The frame every sidebar page sits in, so they share one width, one heading
//! style and one background.

use dioxus::prelude::*;

/// The white panel a page groups related content in.
pub(crate) const CARD_CLASS: &str = "bg-white dark:bg-gray-700 rounded-lg p-4 shadow";
/// A card's heading.
pub(crate) const CARD_TITLE_CLASS: &str = "text-lg font-semibold text-gray-900 dark:text-white";

#[component]
pub(crate) fn Page(
    title: String,
    /// One or two sentences under the title saying what the page is for.
    description: Option<String>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "min-h-screen bg-gray-50 dark:bg-gray-800 px-6 py-8",
            div {
                class: "mx-auto w-full max-w-2xl flex flex-col gap-4",
                header {
                    class: "flex flex-col gap-1",
                    h1 { class: "text-2xl font-bold text-gray-900 dark:text-white", {title} }
                    if let Some(description) = description {
                        p { class: "text-sm text-gray-600 dark:text-gray-300", {description} }
                    }
                }
                {children}
            }
        }
    }
}
