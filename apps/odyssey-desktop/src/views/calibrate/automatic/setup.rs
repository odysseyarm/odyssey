//! Step 1: which screen, which camera, and optionally how big the screen is.

use dioxus::prelude::*;
use odyssey_common::MAX_SCREEN_ID;

use super::screens::screen_label;
use super::ui::{StepPage, CARD, PRIMARY_BUTTON, PRIMARY_ID};
use super::wizard::{CameraList, Wizard};
use crate::components::wizard::{Icon, IconView, Spinner};
use crate::components::ScreenPicker;

#[component]
pub fn SetupStep(wizard: Wizard) -> Element {
    // Cameras are only looked for while this step is on screen; the polling
    // stops when the component goes away.
    use_future(move || async move { wizard.poll_cameras().await });

    let can_continue = wizard.can_continue();
    let notes = wizard.notes.read().clone();

    rsx! {
        StepPage {
            title: "Let's calibrate a screen",
            subtitle: "Three quick questions, then we'll point the camera at the screen.",
            footer: rsx! {
                span {}
                button {
                    id: PRIMARY_ID,
                    class: PRIMARY_BUTTON,
                    r#type: "button",
                    disabled: !can_continue,
                    onclick: move |_| wizard.begin_position(),
                    "Continue"
                }
            },

            for note in notes {
                div {
                    key: "{note}",
                    role: "alert",
                    class: "rounded-xl border-2 border-amber-300 bg-amber-50 px-4 py-3 text-base text-amber-900 dark:border-amber-700 dark:bg-amber-950/60 dark:text-amber-100",
                    "{note}"
                }
            }

            section {
                class: "{CARD} flex flex-col gap-4",
                h2 { class: "text-xl font-semibold text-gray-900 dark:text-white", "1. Which screen?" }
                ScreenPicker { selection: wizard.monitors }
                ScreenNumberPicker { wizard }
            }

            section {
                class: "{CARD} flex flex-col gap-3",
                h2 { class: "text-xl font-semibold text-gray-900 dark:text-white", "2. Which camera?" }
                CameraPicker { wizard }
            }

            section {
                class: "{CARD} flex flex-col gap-2",
                h2 { class: "text-xl font-semibold text-gray-900 dark:text-white", "3. How big is the screen? (optional)" }
                p {
                    class: "text-base text-gray-600 dark:text-gray-300",
                    "If you know the diagonal, type it in for a more exact result. Otherwise leave it empty and the camera will measure it."
                }
                DiagonalField { wizard }
            }
        }
    }
}

#[component]
fn ScreenNumberPicker(wizard: Wizard) -> Element {
    let chosen = wizard.screen_id();
    let saved = *wizard.saved.read();
    rsx! {
        div {
            class: "flex flex-col gap-2",
            span {
                id: "screen-number-label",
                class: "text-base font-medium text-gray-500 dark:text-gray-400",
                "Save the result as:"
            }
            div {
                role: "radiogroup",
                aria_labelledby: "screen-number-label",
                class: "grid grid-cols-3 gap-2 sm:grid-cols-6",
                for id in 0..=MAX_SCREEN_ID {
                    {
                        let selected = id == chosen;
                        let taken = saved[usize::from(id)];
                        let class = if selected {
                            "border-blue-600 bg-blue-600 text-white"
                        } else {
                            "border-gray-300 bg-white text-gray-800 hover:border-blue-400 dark:border-gray-500 dark:bg-gray-800 dark:text-gray-100"
                        };
                        let status_class = if selected { "opacity-90" } else { "text-gray-500 dark:text-gray-400" };
                        rsx! {
                            button {
                                key: "{id}",
                                r#type: "button",
                                role: "radio",
                                aria_checked: selected,
                                class: "flex flex-col items-center rounded-lg border-2 px-2 py-2 focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:focus-visible:ring-blue-800 {class}",
                                onclick: move |_| *wizard.screen_choice.write() = Some(id),
                                span { class: "text-base font-semibold", "{screen_label(id)}" }
                                span {
                                    class: "text-xs {status_class}",
                                    if taken { "Calibrated" } else { "Empty" }
                                }
                            }
                        }
                    }
                }
            }
            if saved[usize::from(chosen)] {
                span {
                    class: "text-sm text-amber-700 dark:text-amber-300",
                    "{screen_label(chosen)} already has a calibration. You'll be asked before it is replaced."
                }
            }
        }
    }
}

#[component]
fn CameraPicker(wizard: Wizard) -> Element {
    let cameras = wizard.cameras.read().clone();
    let chosen = wizard.camera();

    match cameras {
        CameraList::Searching => rsx! {
            div {
                class: "flex items-center gap-3 text-base text-gray-600 dark:text-gray-300",
                Spinner { class: "size-5".to_string() }
                "Looking for cameras…"
            }
        },
        CameraList::Failed(error) => rsx! {
            NoCamera {
                title: "Couldn't look for cameras".to_string(),
                detail: "Unplug the camera and plug it back in. If this keeps happening, restart the app.".to_string(),
                technical: error,
            }
        },
        CameraList::Found(list) if list.is_empty() => {
            if cfg!(feature = "orbbec") || wizard.demo {
                rsx! {
                    NoCamera {
                        title: "No camera found".to_string(),
                        detail: "Plug in the Orbbec camera with its USB cable. This page notices by itself.".to_string(),
                    }
                }
            } else {
                rsx! {
                    NoCamera {
                        title: "This version has no camera support".to_string(),
                        detail: "Install the full version of the app to calibrate with a camera.".to_string(),
                    }
                }
            }
        }
        CameraList::Found(list) => rsx! {
            div {
                role: "radiogroup",
                aria_label: "Camera",
                class: "flex flex-col gap-2",
                for camera in list {
                    {
                        let selected = chosen.as_ref().is_some_and(|c| c.id == camera.id);
                        let class = if selected {
                            "border-blue-600 bg-blue-50 dark:bg-blue-950/50 dark:border-blue-400"
                        } else {
                            "border-gray-300 bg-white hover:border-blue-400 dark:border-gray-500 dark:bg-gray-800"
                        };
                        let id = camera.id.clone();
                        rsx! {
                            button {
                                key: "{camera.id}",
                                r#type: "button",
                                role: "radio",
                                aria_checked: selected,
                                class: "flex items-center gap-3 rounded-lg border-2 px-4 py-3 text-left focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:focus-visible:ring-blue-800 {class}",
                                onclick: move |_| *wizard.camera_choice.write() = Some(id.clone()),
                                IconView { icon: Icon::Camera, class: "size-7 shrink-0 text-blue-600 dark:text-blue-400".to_string() }
                                span {
                                    class: "flex min-w-0 flex-col",
                                    span { class: "text-lg font-medium text-gray-900 dark:text-white", "{camera.name}" }
                                    if !camera.serial.is_empty() {
                                        span { class: "text-sm text-gray-500 dark:text-gray-400", "Serial {camera.serial}" }
                                    }
                                }
                                if selected {
                                    IconView { icon: Icon::Check, class: "ms-auto size-6 text-blue-600 dark:text-blue-400".to_string() }
                                }
                            }
                        }
                    }
                }
            }
        },
    }
}

#[component]
fn NoCamera(title: String, detail: String, technical: Option<String>) -> Element {
    rsx! {
        div {
            class: "flex flex-col items-center gap-2 rounded-xl border-2 border-dashed border-gray-300 px-4 py-6 text-center dark:border-gray-500",
            IconView { icon: Icon::Camera, class: "size-10 text-gray-400".to_string() }
            div { class: "text-xl font-semibold text-gray-900 dark:text-white", "{title}" }
            div { class: "text-base text-gray-600 dark:text-gray-300", "{detail}" }
            if let Some(technical) = technical {
                details {
                    class: "text-sm text-gray-500 dark:text-gray-400",
                    summary { class: "cursor-pointer", "Details" }
                    code { class: "break-all", "{technical}" }
                }
            }
        }
    }
}

#[component]
fn DiagonalField(wizard: Wizard) -> Element {
    let error = wizard.diagonal().err().map(|e| e.message());
    rsx! {
        div {
            class: "flex flex-col gap-1",
            label {
                r#for: "diagonal",
                class: "text-base font-medium text-gray-900 dark:text-white",
                "Screen size (diagonal)"
            }
            div {
                class: "flex items-center gap-2",
                input {
                    id: "diagonal",
                    r#type: "text",
                    inputmode: "decimal",
                    autocomplete: "off",
                    placeholder: "Measure it for me",
                    class: "w-48 rounded-lg border border-gray-300 bg-white px-3 py-2 text-lg text-gray-900 focus:border-blue-500 focus:outline-none focus:ring-4 focus:ring-blue-200 dark:border-gray-500 dark:bg-gray-800 dark:text-white dark:focus:ring-blue-900",
                    value: "{wizard.diagonal_text}",
                    oninput: move |e| *wizard.diagonal_text.write() = e.value(),
                }
                span { class: "text-lg text-gray-600 dark:text-gray-300", "inches" }
            }
            if let Some(error) = error {
                span { role: "alert", class: "text-sm text-red-700 dark:text-red-300", "{error}" }
            }
        }
    }
}
