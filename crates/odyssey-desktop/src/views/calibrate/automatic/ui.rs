//! Layout pieces and styles shared by the wizard's step views.

use dioxus::prelude::*;

pub(super) const PRIMARY_BUTTON: &str = "btn btn-primary btn-lg";
pub(super) const SECONDARY_BUTTON: &str = "btn btn-secondary btn-lg";
pub(super) const CARD: &str = "card";

/// A text-only button that reads as a link.
pub(super) const LINK_BUTTON: &str = "link text-base";

/// A one-line warning box.
pub(super) const AMBER_ALERT: &str = "alert alert-warning text-base";

/// A spinner beside what is being waited for.
pub(super) const WORKING_ROW: &str = "flex items-center gap-3 text-base text-fg-muted";

/// A one-line hint box with an icon.
pub(super) const INFO_NOTE: &str = "alert alert-info flex items-start gap-3 text-base";

/// The empty stage where the live camera picture goes.
pub(super) const CAMERA_PLACEHOLDER: &str =
    "stage flex aspect-video w-full items-center justify-center";

/// Element id of the step's main button, so the wizard can focus it and Enter
/// does the obvious thing.
pub(super) const PRIMARY_ID: &str = "primary-action";

/// Moves keyboard focus to the main button.
pub(super) fn focus_primary() {
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
                h1 { class: "text-3xl font-bold text-fg-strong", {title} }
                if let Some(subtitle) = subtitle {
                    p { class: "mt-1 text-lg text-fg-muted", {subtitle} }
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
            class: "group rounded-xl border border-line bg-surface-alt px-4 py-3",
            summary {
                class: "cursor-pointer select-none text-base font-medium text-fg",
                {summary}
            }
            div { class: "mt-3 flex flex-col gap-2 text-sm text-fg", {children} }
        }
    }
}

/// A label and value on one line inside [`Details`].
#[component]
pub fn Fact(label: String, value: String) -> Element {
    rsx! {
        div {
            class: "flex justify-between gap-4",
            span { class: "text-fg-muted", {label} }
            span { class: "text-right tabular-nums", {value} }
        }
    }
}
