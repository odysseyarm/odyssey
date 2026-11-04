//! The boxes the review shows above and below the camera picture.

use dioxus::prelude::*;
use odyssey_autocalib::session::SizeCheck;

use crate::components::wizard::{Icon, IconView};
use crate::views::calibrate::automatic::kind::SizeOrigin;
use crate::views::calibrate::automatic::quality::disagreement_message;
use crate::views::calibrate::automatic::ui::{INFO_NOTE, PRIMARY_BUTTON, SECONDARY_BUTTON};
use crate::views::calibrate::automatic::wizard::Wizard;

const AMBER_BOX: &str = "flex flex-col gap-3 rounded-xl border-2 border-amber-300 bg-amber-50 p-4 text-amber-900 dark:border-amber-700 dark:bg-amber-950/60 dark:text-amber-100";

/// The camera and the declared size disagree: what that means and what to do.
#[component]
pub(super) fn DisagreementNotice(
    wizard: Wizard,
    check: SizeCheck,
    size_origin: Option<SizeOrigin>,
    can_use_measured: bool,
) -> Element {
    let saving = *wizard.saving.read();
    rsx! {
        div {
            role: "alert",
            class: AMBER_BOX,
            div {
                class: "flex items-start gap-3",
                IconView { icon: Icon::Warning, class: "mt-0.5 size-6 shrink-0 text-amber-600 dark:text-amber-400".to_string() }
                div {
                    class: "flex flex-col gap-1",
                    div { class: "text-lg font-semibold", "The camera and the screen's size don't agree" }
                    div { class: "text-base", "{disagreement_message(&check, size_origin)}" }
                }
            }
            if can_use_measured {
                div {
                    button {
                        class: SECONDARY_BUTTON,
                        r#type: "button",
                        disabled: saving,
                        onclick: move |_| *wizard.use_measured.write() = true,
                        "Use the camera's measurement instead"
                    }
                }
                details {
                    class: "text-sm",
                    summary { class: "cursor-pointer select-none font-medium", "What's the difference?" }
                    p {
                        class: "mt-2",
                        "A size from the TV or the box is exact if the TV shows its whole picture. The camera's own measurement is usually within about half a percent of the distance, so a few millimeters per meter, which makes it a good second opinion but slightly less exact. If the TV crops its picture, the camera's measurement is the one that matches what you see."
                    }
                }
            }
            div {
                class: "flex flex-col gap-1",
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    disabled: saving,
                    onclick: move |_| wizard.check_by_hand(),
                    if size_origin == Some(SizeOrigin::Reference) {
                        "Measure by hand again"
                    } else {
                        "Settle it with a tape measure or credit card"
                    }
                }
                span {
                    class: "text-sm",
                    "Goes back to Setup with a tape measure line on the screen. About a minute, then measure again."
                }
            }
        }
    }
}

/// The camera sized a screen that hides the edge of its picture.
#[component]
pub(super) fn OverscanNotice() -> Element {
    rsx! {
        div {
            class: INFO_NOTE,
            IconView { icon: Icon::Crop, class: "mt-0.5 size-6 shrink-0 text-sky-600 dark:text-sky-300".to_string() }
            span {
                "This screen hides the edge of its picture. The size above is the whole picture the computer draws, including the part you can't see, and the calibration is for that picture. Turning off overscan in the TV's picture settings ('Just Scan' or 'PC mode') makes all of it visible, which also lets the TV's own size be used."
            }
        }
    }
}

/// The review is showing the camera's own size instead of the declared one.
#[component]
pub(super) fn UsingMeasuredNotice(wizard: Wizard) -> Element {
    let saving = *wizard.saving.read();
    rsx! {
        div {
            class: "flex flex-wrap items-center justify-between gap-3 rounded-xl bg-sky-50 px-4 py-3 text-base text-sky-900 dark:bg-sky-950/50 dark:text-sky-100",
            span { "Using the camera's own measurement instead of the declared size." }
            button {
                class: SECONDARY_BUTTON,
                r#type: "button",
                disabled: saving,
                onclick: move |_| *wizard.use_measured.write() = false,
                "Go back to the declared size"
            }
        }
    }
}

/// Saving would overwrite a calibration that is already stored for the screen.
#[component]
pub(super) fn ReplaceConfirmation(wizard: Wizard, screen: String) -> Element {
    let saving = *wizard.saving.read();
    rsx! {
        div {
            role: "alertdialog",
            aria_label: "Replace the existing calibration?",
            class: AMBER_BOX,
            div { class: "text-lg font-semibold", "{screen} already has a calibration" }
            div { class: "text-base", "Saving replaces it. The old calibration can't be brought back." }
            div {
                class: "flex gap-3",
                button {
                    class: PRIMARY_BUTTON,
                    r#type: "button",
                    disabled: saving,
                    onclick: move |_| wizard.save(true),
                    "Replace it"
                }
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    disabled: saving,
                    onclick: move |_| *wizard.confirm_replace.write() = false,
                    "Keep the old one"
                }
            }
        }
    }
}

#[component]
pub(super) fn SaveError(error: String) -> Element {
    rsx! {
        div {
            role: "alert",
            class: "rounded-xl border-2 border-red-300 bg-red-50 px-4 py-3 text-base text-red-900 dark:border-red-700 dark:bg-red-950/60 dark:text-red-100",
            "Couldn't save the calibration: {error}"
        }
    }
}
