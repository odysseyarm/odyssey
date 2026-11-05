//! The frame every sidebar page sits in, so they share one width, one heading
//! style and one background.

use dioxus::prelude::*;

#[component]
pub(crate) fn Page(
    title: String,
    /// One or two sentences under the title saying what the page is for.
    description: Option<String>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "min-h-screen bg-canvas px-6 py-8",
            div {
                class: "mx-auto w-full max-w-2xl flex flex-col gap-4",
                header {
                    class: "flex flex-col gap-1",
                    h1 { class: "text-2xl", {title} }
                    if let Some(description) = description {
                        p { class: "text-sm text-fg-muted", {description} }
                    }
                }
                {children}
            }
        }
    }
}
