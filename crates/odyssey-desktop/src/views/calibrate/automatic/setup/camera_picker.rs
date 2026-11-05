//! Choosing the camera, and what to show when there is none.

use dioxus::prelude::*;

use crate::components::wizard::{Icon, IconView, Spinner};
use crate::sdk_install::{self, Delivery};
use crate::views::calibrate::automatic::camera_support::{self, Notice, SDK_DOWNLOAD_URL};
use crate::views::calibrate::automatic::sdk_download::DownloadSdk;
use crate::views::calibrate::automatic::ui::{
    AMBER_ALERT, PRIMARY_BUTTON, SECONDARY_BUTTON, WORKING_ROW,
};
use crate::views::calibrate::automatic::wizard::{CameraList, Wizard};

#[component]
pub(super) fn CameraPicker(wizard: Wizard) -> Element {
    let cameras = wizard.cameras.read().clone();
    let chosen = wizard.camera();

    match cameras {
        CameraList::Searching => rsx! {
            div {
                class: WORKING_ROW,
                Spinner { class: "size-5".to_string() }
                "Looking for cameras…"
            }
        },
        CameraList::Failed(error) => rsx! {
            NoCamera {
                title: "Couldn't look for cameras".to_string(),
                detail: "Unplug the camera and plug it back in. If this keeps happening, restart the app.".to_string(),
                technical: vec![error],
            }
        },
        CameraList::Unavailable(status) => match camera_support::notice(&status) {
            Some(notice) => rsx! { CameraSupportNotice { wizard, notice } },
            None => rsx! {},
        },
        CameraList::Found(list) if list.is_empty() => rsx! {
            NoCamera {
                title: "No camera found".to_string(),
                detail: "Plug in the Orbbec camera with its USB cable. This page notices by itself.".to_string(),
                technical: Vec::new(),
            }
        },
        CameraList::Found(list) => rsx! {
            div {
                role: "radiogroup",
                aria_label: "Camera",
                class: "flex flex-col gap-2",
                for camera in list {
                    {
                        let selected = chosen.as_ref().is_some_and(|c| c.id == camera.id);
                        let class = if selected {
                            "border-accent-ink bg-accent-soft"
                        } else {
                            "border-line bg-surface hover:border-accent-ink"
                        };
                        let id = camera.id.clone();
                        rsx! {
                            button {
                                key: "{camera.id}",
                                r#type: "button",
                                role: "radio",
                                aria_checked: selected,
                                class: "flex items-center gap-3 rounded-lg border-2 px-4 py-3 text-left {class}",
                                onclick: move |_| *wizard.camera_choice.write() = Some(id.clone()),
                                IconView { icon: Icon::Camera, class: "size-7 shrink-0 text-accent-ink".to_string() }
                                span {
                                    class: "flex min-w-0 flex-col",
                                    span { class: "text-lg font-medium text-fg-strong", "{camera.name}" }
                                    if !camera.serial.is_empty() {
                                        span { class: "text-sm text-fg-muted", "Serial {camera.serial}" }
                                    }
                                }
                                if selected {
                                    IconView { icon: Icon::Check, class: "ms-auto size-6 text-accent-ink".to_string() }
                                }
                            }
                            if let Some(warning) = camera.link.warning() {
                                div {
                                    key: "{camera.id}-link",
                                    role: "alert",
                                    class: AMBER_ALERT,
                                    {warning}
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
pub(super) fn NoCamera(
    title: String,
    detail: String,
    technical: Vec<String>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "flex flex-col items-center gap-2 rounded-xl border-2 border-dashed border-line px-4 py-6 text-center",
            IconView { icon: Icon::Camera, class: "size-10 text-fg-faint".to_string() }
            div { class: "text-xl font-semibold text-fg-strong", {title} }
            div { class: "text-base text-fg-muted", {detail} }
            {children}
            if !technical.is_empty() {
                details {
                    class: "w-full text-left text-sm text-fg-muted",
                    summary { class: "cursor-pointer text-center", "Details" }
                    div {
                        class: "mt-2 flex flex-col gap-1",
                        for (i, line) in technical.iter().enumerate() {
                            code { key: "{i}", class: "break-words", {line.as_str()} }
                        }
                    }
                }
            }
        }
    }
}

/// The camera library is missing, broken or the wrong version. The page keeps
/// checking by itself; the button only saves the wait.
#[component]
pub(super) fn CameraSupportNotice(wizard: Wizard, notice: Notice) -> Element {
    rsx! {
        NoCamera {
            title: notice.title,
            detail: notice.detail,
            technical: notice.details,
            div {
                class: "flex flex-wrap items-center justify-center gap-3 pt-2",
                if notice.offer_download {
                    match sdk_install::delivery() {
                        Delivery::Link => rsx! {
                            a { class: PRIMARY_BUTTON, href: SDK_DOWNLOAD_URL, "Get the Orbbec SDK" }
                        },
                        Delivery::Download => rsx! { DownloadSdk { wizard } },
                        Delivery::Installer => rsx! {
                            p {
                                class: "max-w-xl text-sm text-fg-muted",
                                "This copy of Odyssey doesn't download the camera software. To add Orbbec camera support, run the \
                                 Odyssey installer again, choose Change, and tick \"Orbbec camera support\"."
                            }
                        },
                        Delivery::Offline => rsx! {
                            p {
                                class: "max-w-xl text-sm text-fg-muted",
                                "Offline mode is on, so Odyssey won't download the camera software. Install it from \
                                 the full Odyssey installer (the \"Orbbec camera support\" option), or ask your \
                                 administrator."
                            }
                        },
                    }
                }
                button {
                    r#type: "button",
                    class: SECONDARY_BUTTON,
                    onclick: move |_| wizard.request_camera_list(),
                    "Check again"
                }
            }
        }
    }
}
