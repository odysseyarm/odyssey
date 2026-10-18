//! Offers to download Orbbec's camera software, after the user has accepted
//! Orbbec's licence, to remove a copy that was downloaded, and to say when the
//! copy in use is not the version this app is tested with.
//!
//! Only builds with the `sdk-download` feature contain the download. Builds for
//! customers who may only run offline software leave it out, and
//! `sdk_install::delivery` tells Setup to point at the installer instead.

use dioxus::prelude::*;

use super::ui::SECONDARY_BUTTON;
use super::wizard::{CameraList, Wizard};
use crate::sdk_install::{self, Advisory, Delivery, Removal};

#[cfg(feature = "sdk-download")]
mod online {
    use dioxus::prelude::*;

    use super::super::ui::PRIMARY_BUTTON;
    use super::super::wizard::Wizard;
    use crate::components::wizard::Spinner;
    use crate::sdk_install::{self, Installed, Phase};

    #[derive(Clone, PartialEq)]
    enum State {
        Ready,
        Working(Phase),
        /// Downloaded, but the copy in use cannot be replaced until Odyssey restarts.
        Waiting,
        Failed(String),
    }

    /// What the progress line says.
    fn describe(phase: Phase) -> String {
        match phase {
            Phase::Downloading { received, total } if total > 0 => {
                format!("Downloading… {}%", received.saturating_mul(100) / total)
            }
            Phase::Downloading { .. } => "Downloading…".to_string(),
            Phase::Verifying => "Checking the download…".to_string(),
            Phase::Installing => "Installing…".to_string(),
        }
    }

    #[component]
    pub fn DownloadSdk(wizard: Wizard) -> Element {
        let mut accepted = use_signal(|| false);
        let mut state = use_signal(|| State::Ready);
        let pin = sdk_install::pin();
        let megabytes = pin.windows_x64.size / 1_000_000;
        let working = matches!(*state.read(), State::Working(_));

        rsx! {
            div {
                class: "flex max-w-xl flex-col items-center gap-3 pt-2",
                p {
                    class: "text-sm text-gray-600 dark:text-gray-300",
                    "Odyssey can download Orbbec SDK {pin.version} for you (about {megabytes} MB). \
                     Orbbec licenses it for use with Orbbec cameras under their End User License Agreement."
                }
                a {
                    class: "text-sm text-blue-700 underline dark:text-blue-300",
                    href: "{pin.eula.url}",
                    "Read the licence"
                }
                label {
                    class: "flex items-center gap-2 text-base text-gray-900 dark:text-white",
                    input {
                        r#type: "checkbox",
                        class: "size-5",
                        checked: accepted(),
                        disabled: working,
                        onchange: move |e| accepted.set(e.checked()),
                    }
                    "I accept Orbbec's End User License Agreement"
                }
                match state() {
                    State::Working(phase) => rsx! {
                        div {
                            class: "flex items-center gap-3 text-base text-gray-600 dark:text-gray-300",
                            Spinner { class: "size-5".to_string() }
                            "{describe(phase)}"
                        }
                    },
                    State::Waiting => rsx! {
                        div {
                            role: "status",
                            class: "text-sm text-gray-700 dark:text-gray-200",
                            "Downloaded. Restart Odyssey to switch to it."
                        }
                    },
                    State::Failed(error) => rsx! {
                        div {
                            role: "alert",
                            class: "text-sm text-red-700 dark:text-red-300",
                            "The download didn't work: {error}"
                        }
                    },
                    State::Ready => rsx! {},
                }
                button {
                    r#type: "button",
                    class: PRIMARY_BUTTON,
                    disabled: !accepted() || working,
                    onclick: move |_| {
                        state.set(State::Working(Phase::Downloading { received: 0, total: 0 }));
                        spawn(async move {
                            match sdk_install::install(|phase| state.set(State::Working(phase))).await {
                                Ok(Installed::Ready) => {
                                    state.set(State::Ready);
                                    wizard.request_camera_list();
                                }
                                Ok(Installed::AfterRestart) => state.set(State::Waiting),
                                Err(error) => state.set(State::Failed(format!("{error:#}"))),
                            }
                        });
                    },
                    "Download Orbbec SDK {pin.version}"
                }
            }
        }
    }
}

#[cfg(feature = "sdk-download")]
pub use online::DownloadSdk;

/// Without the feature there is nothing to download; Setup never asks for this.
#[cfg(not(feature = "sdk-download"))]
#[component]
pub fn DownloadSdk(wizard: Wizard) -> Element {
    let _ = wizard;
    rsx! {}
}

const NOTE_CLASS: &str = "flex flex-col gap-2 rounded-xl border-2 border-amber-300 bg-amber-50 px-4 py-3 text-base text-amber-900 dark:border-amber-700 dark:bg-amber-950/60 dark:text-amber-100";

/// Says so when the camera software in use is not the version this app is tested
/// with. Only a warning: the loader accepts 2.8 and newer, so the cameras still
/// work. When the copy is one Odyssey downloaded and it is older, the pinned one
/// can be downloaded in its place; for a copy found elsewhere, or a newer one,
/// there is nothing to offer.
#[component]
pub fn SdkVersionNote(wizard: Wizard) -> Element {
    // Cameras were listed, so the library is loaded and the lookup below is cached.
    if !matches!(*wizard.cameras.read(), CameraList::Found(_)) {
        return rsx! {};
    }
    let Some(advisory) = sdk_install::current_advisory() else {
        return rsx! {};
    };

    match advisory {
        Advisory::Older {
            found,
            pinned,
            replaceable,
        } => rsx! {
            div {
                role: "note",
                class: NOTE_CLASS,
                span {
                    class: "font-semibold",
                    "The camera software is older than this app is tested with"
                }
                span {
                    "Orbbec SDK {found} is in use and Odyssey is tested with {pinned}. \
                     The camera should still work, but update it if you run into problems."
                }
                if replaceable && sdk_install::delivery() == Delivery::Download {
                    DownloadSdk { wizard }
                } else if !replaceable {
                    span {
                        class: "text-sm",
                        "Odyssey didn't install it, so update it where you got it."
                    }
                }
            }
        },
        Advisory::Newer { found, pinned } => rsx! {
            div {
                role: "note",
                class: NOTE_CLASS,
                span {
                    class: "font-semibold",
                    "The camera software is newer than this app is tested with"
                }
                span {
                    "Orbbec SDK {found} is in use and Odyssey is tested with {pinned}. \
                     It will probably work, but if the camera misbehaves, install {pinned}."
                }
            }
        },
    }
}

/// A line under the camera list for a copy of the SDK this app downloaded, with a
/// way to delete it. Shows nothing otherwise, including for the installer's copy,
/// which Windows removes with the app.
#[component]
pub fn DownloadedSdkRow(wizard: Wizard) -> Element {
    let mut error = use_signal(|| None::<String>);
    if sdk_install::delivery() != Delivery::Download || !sdk_install::has_managed_copy() {
        return rsx! {};
    }
    let removal_pending = sdk_install::removal_pending();
    let update_pending = sdk_install::update_pending();

    rsx! {
        div {
            class: "flex flex-wrap items-center gap-3 text-sm text-gray-500 dark:text-gray-400",
            if removal_pending {
                "The camera software Odyssey downloaded will be removed the next time Odyssey starts."
            } else {
                if update_pending {
                    "Camera software downloaded by Odyssey. A newer copy takes over the next time Odyssey starts."
                } else {
                    "Camera software downloaded by Odyssey."
                }
                button {
                    r#type: "button",
                    class: SECONDARY_BUTTON,
                    onclick: move |_| {
                        match sdk_install::remove_managed_copy() {
                            // Either way the row redraws: gone, or marked as pending.
                            Ok(Removal::Removed | Removal::AfterRestart) => error.set(None),
                            Err(e) => error.set(Some(format!("Couldn't remove it: {e}"))),
                        }
                        wizard.request_camera_list();
                    },
                    "Remove"
                }
            }
            if let Some(message) = error() {
                span { role: "alert", class: "text-red-700 dark:text-red-300", "{message}" }
            }
        }
    }
}
