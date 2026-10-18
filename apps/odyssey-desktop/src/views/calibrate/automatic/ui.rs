//! Layout pieces and styles shared by the wizard's step views.

use dioxus::prelude::*;

pub const PRIMARY_BUTTON: &str = "inline-flex items-center justify-center gap-2 rounded-lg bg-blue-600 px-7 py-3 text-lg font-semibold text-white shadow hover:bg-blue-700 focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:focus-visible:ring-blue-800 disabled:opacity-40 disabled:pointer-events-none";
pub const SECONDARY_BUTTON: &str = "inline-flex items-center justify-center gap-2 rounded-lg border border-gray-300 bg-white px-5 py-3 text-base font-medium text-gray-800 hover:bg-gray-100 focus:outline-none focus-visible:ring-4 focus-visible:ring-gray-300 dark:border-gray-500 dark:bg-gray-700 dark:text-gray-100 dark:hover:bg-gray-600 dark:focus-visible:ring-gray-600 disabled:opacity-40 disabled:pointer-events-none";
pub const CARD: &str = "rounded-xl bg-white p-5 shadow dark:bg-gray-700";

/// Element id of the step's main button, so the wizard can focus it and Enter
/// does the obvious thing.
pub const PRIMARY_ID: &str = "primary-action";

/// Moves keyboard focus to the main button.
pub fn focus_primary() {
    let _ = document::eval(&format!(
        "document.getElementById('{PRIMARY_ID}')?.focus();"
    ));
}

/// One step's page: a headline, one line of instruction, the content and a
/// footer row of buttons.
#[component]
pub fn StepPage(
    title: String,
    subtitle: Option<String>,
    children: Element,
    footer: Element,
) -> Element {
    rsx! {
        div {
            class: "flex flex-col gap-5",
            div {
                h1 { class: "text-3xl font-bold text-gray-900 dark:text-white", "{title}" }
                if let Some(subtitle) = subtitle {
                    p { class: "mt-1 text-lg text-gray-600 dark:text-gray-300", "{subtitle}" }
                }
            }
            {children}
            div { class: "flex items-center justify-between gap-3 pt-2", {footer} }
        }
    }
}

/// Technical information, collapsed until asked for.
#[component]
pub fn Details(summary: Option<String>, children: Element) -> Element {
    let summary = summary.unwrap_or_else(|| "Details".into());
    rsx! {
        details {
            class: "group rounded-xl border border-gray-200 bg-white/60 px-4 py-3 dark:border-gray-600 dark:bg-gray-700/50",
            summary {
                class: "cursor-pointer select-none text-base font-medium text-gray-700 dark:text-gray-200",
                "{summary}"
            }
            div { class: "mt-3 flex flex-col gap-2 text-sm text-gray-700 dark:text-gray-200", {children} }
        }
    }
}

/// A label and value on one line inside [`Details`].
#[component]
pub fn Fact(label: String, value: String) -> Element {
    rsx! {
        div {
            class: "flex justify-between gap-4",
            span { class: "text-gray-500 dark:text-gray-400", "{label}" }
            span { class: "text-right tabular-nums", "{value}" }
        }
    }
}
