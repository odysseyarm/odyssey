//! The size question for a flat screen, answered by the display, the tech or
//! the camera.

use dioxus::prelude::*;

use super::size_box::SizeBox;
use crate::components::wizard::{Icon, IconView, Spinner};
use crate::views::calibrate::automatic::kind::Finding;
use crate::views::calibrate::automatic::ui::{INFO_NOTE, LINK_BUTTON, WORKING_ROW};
use crate::views::calibrate::automatic::wizard::Wizard;

const FOUND_ICON: &str = "mt-0.5 size-6 shrink-0 text-success-ink";
const MISSING_ICON: &str = "mt-0.5 size-6 shrink-0 text-fg-muted";

#[component]
pub(super) fn FlatSize(wizard: Wizard) -> Element {
    let Some(finding) = wizard.finding.read().clone() else {
        return rsx! {
            div {
                class: WORKING_ROW,
                Spinner { class: "size-5".to_string() }
                "Reading the screen's size…"
            }
        };
    };
    // The size field is always there when the display could not say its size,
    // and behind "Not right?" when it could.
    let field_open = !finding.is_found() || *wizard.manual_size.read();
    let by_hand = wizard.reference().is_some();
    let found = finding.is_found();
    let lines = finding.detail_lines();

    rsx! {
        div {
            class: "flex flex-col gap-3",
            FindingSummary { wizard, finding }

            if found && !field_open {
                div {
                    button {
                        class: LINK_BUTTON,
                        r#type: "button",
                        onclick: move |_| *wizard.manual_size.write() = true,
                        "Not right? Enter the size yourself"
                    }
                }
            }
            if found && !field_open && !by_hand {
                PreferCameraSizeToggle { wizard }
            }
            if field_open {
                SizeBox {
                    wizard,
                    label: if found { "Screen size (diagonal)" } else { "Size printed on the box (optional)" },
                }
                if found {
                    div {
                        button {
                            class: LINK_BUTTON,
                            r#type: "button",
                            onclick: move |_| {
                                wizard.size_text.write().clear();
                                *wizard.manual_size.write() = false;
                            },
                            "Use the TV's own size instead"
                        }
                    }
                }
            }

            div {
                class: INFO_NOTE,
                IconView { icon: Icon::Bulb, class: "mt-0.5 size-6 shrink-0 text-info-ink".to_string() }
                span {
                    span { class: "font-semibold", "Before you start: " }
                    "set the TV to Game or PC mode, or turn on Just Scan, so it doesn't crop the picture."
                }
            }

            details {
                class: "text-sm text-fg-muted",
                summary { class: "cursor-pointer select-none font-medium", "Details" }
                ul {
                    class: "mt-2 flex flex-col gap-1",
                    for line in lines {
                        li { key: "{line}", {line} }
                    }
                }
            }
        }
    }
}

/// What the display said about its size, or which size is used instead.
#[component]
fn FindingSummary(wizard: Wizard, finding: Finding) -> Element {
    let found = finding.is_found();
    let typed = !wizard.size_text.read().trim().is_empty();
    let by_hand = wizard.reference().is_some();
    let prefer_camera = wizard.prefer_camera_size();
    let icon_class = if found { FOUND_ICON } else { MISSING_ICON };
    rsx! {
        div {
            class: "flex items-start gap-3",
            IconView {
                icon: if found { Icon::Check } else { Icon::Viewfinder },
                class: icon_class.to_string(),
            }
            div {
                class: "flex flex-col gap-1",
                p {
                    class: "text-lg font-medium text-fg-strong",
                    if typed && found {
                        "Using the size you entered instead of the TV's."
                    } else if by_hand {
                        "Using your tape or card measurement instead of the TV's."
                    } else if prefer_camera && found {
                        "The camera will measure the screen's size."
                    } else {
                        "{finding.headline()}"
                    }
                }
                if let Some(advice) = finding.advice() {
                    p { class: "text-base text-fg-muted", {advice} }
                }
                if let Some(aside) = finding.projector_aside() {
                    p { class: "text-base text-fg-muted", {aside} }
                }
            }
        }
    }
}

/// The remembered choice to size flat screens with the camera instead of by
/// what the display reports.
#[component]
fn PreferCameraSizeToggle(wizard: Wizard) -> Element {
    let prefer_camera = wizard.prefer_camera_size();
    rsx! {
        label {
            r#for: "prefer-camera-size",
            class: "flex w-fit items-start gap-3 text-base text-fg-strong",
            input {
                id: "prefer-camera-size",
                r#type: "checkbox",
                class: "mt-1 size-5",
                checked: prefer_camera,
                onchange: move |e| wizard.set_prefer_camera_size(e.checked()),
            }
            span {
                "Measure the size with the camera instead of using the TV's own"
                span {
                    class: "block text-sm text-fg-muted",
                    "Remembered for every screen. A size you type still wins."
                }
            }
        }
    }
}
