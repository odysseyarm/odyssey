//! Step 1: what is being calibrated, which screen, which camera, and how big
//! it is (known from the display, typed in, or left to the camera).

use dioxus::prelude::*;

use super::camera_picker::CameraPicker;
use super::kind_chooser::KindChooser;
use super::screen_number_picker::ScreenNumberPicker;
use super::size_section::SizeSection;
use crate::components::ScreenPicker;
use crate::views::calibrate::automatic::kind::DisplayKind;
use crate::views::calibrate::automatic::reference_helper::ReferenceHelper;
use crate::views::calibrate::automatic::scale_board_info::ScaleBoardInfo;
use crate::views::calibrate::automatic::sdk_download::{DownloadedSdkRow, SdkVersionNote};
use crate::views::calibrate::automatic::ui::{
    AMBER_ALERT, CARD, PRIMARY_BUTTON, PRIMARY_ID, StepPage,
};
use crate::views::calibrate::automatic::wizard::Wizard;

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
            subtitle: "A few quick questions, then we'll point the camera at the screen.",
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
                    class: AMBER_ALERT,
                    {note}
                }
            }

            section {
                class: "{CARD} flex flex-col gap-4",
                h2 { class: "text-xl font-semibold text-gray-900 dark:text-white", "1. What are you calibrating?" }
                KindChooser { wizard }
            }

            section {
                class: "{CARD} flex flex-col gap-4",
                h2 { class: "text-xl font-semibold text-gray-900 dark:text-white", "2. Which screen?" }
                ScreenPicker { selection: wizard.monitors }
                ScreenNumberPicker { wizard }
            }

            section {
                class: "{CARD} flex flex-col gap-3",
                h2 { class: "text-xl font-semibold text-gray-900 dark:text-white", "3. Which camera?" }
                CameraPicker { wizard }
                SdkVersionNote { wizard }
                DownloadedSdkRow { wizard }
            }

            section {
                class: "{CARD} flex flex-col gap-3",
                h2 {
                    class: "text-xl font-semibold text-gray-900 dark:text-white",
                    match wizard.kind() {
                        DisplayKind::Flat => "4. How big is the screen?",
                        DisplayKind::Projector => "4. How big is the picture? (optional)",
                    }
                }
                SizeSection { wizard }
                ReferenceHelper { wizard }
                details {
                    class: "text-sm text-gray-600 dark:text-gray-300",
                    summary { class: "cursor-pointer select-none font-medium", "Scale board (optional, prototype)" }
                    div {
                        class: "mt-2 flex flex-col gap-2",
                        p { "Print files for the scale board. It does not change your calibration yet." }
                        ScaleBoardInfo {}
                    }
                }
            }
        }
    }
}
