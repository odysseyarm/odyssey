use dioxus::prelude::*;

use super::progress_bar::ProgressBar;
use super::update_manager::{FirmwareUpdateManager, use_polled_state};
use super::update_state::{BYTES_PER_KB, FirmwareUpdateState, percent_complete};

/// Props for the updating device row component
#[derive(Props, Clone, PartialEq)]
pub(crate) struct UpdatingDeviceRowProps {
    pub uuid: [u8; 6],
    pub manager: FirmwareUpdateManager,
}

/// What the row says about `state`: the fixed wording plus any data it carries.
fn state_display(state: &FirmwareUpdateState) -> String {
    match state {
        FirmwareUpdateState::Flashing { progress, total }
        | FirmwareUpdateState::FlashingNetCore { progress, total } => {
            format!(
                "{} {}%",
                state.status_text(),
                percent_complete(*progress, *total)
            )
        }
        FirmwareUpdateState::Failed(error) => format!("Failed: {error}"),
        _ => state.status_text().to_string(),
    }
}

/// A component that displays a device being updated (when not in device list)
#[component]
pub(crate) fn UpdatingDeviceRow(props: UpdatingDeviceRowProps) -> Element {
    let uuid = props.uuid;

    // Poll state from manager
    let current_state = use_polled_state(&props.manager, uuid)();
    let state_display = state_display(&current_state);

    let is_error = matches!(current_state, FirmwareUpdateState::Failed(_));
    let is_complete = matches!(current_state, FirmwareUpdateState::Complete);

    rsx! {
        div {
            class: "flex flex-col p-3 bg-gray-50 dark:bg-gray-600 rounded gap-2",
            div {
                class: "flex items-center justify-between",
                div {
                    class: "flex items-center gap-3",
                    div {
                        class: if is_error { "w-2 h-2 bg-red-500 rounded-full" }
                               else if is_complete { "w-2 h-2 bg-green-500 rounded-full" }
                               else { "w-2 h-2 bg-yellow-500 rounded-full animate-pulse" }
                    }
                    span {
                        class: "text-sm font-mono text-gray-700 dark:text-gray-200",
                        "{uuid[0]:02x}:{uuid[1]:02x}:{uuid[2]:02x}:{uuid[3]:02x}:{uuid[4]:02x}:{uuid[5]:02x}"
                    }
                }
                span {
                    class: if is_error { "text-xs text-red-500" }
                           else if is_complete { "text-xs text-green-500" }
                           else { "text-xs text-yellow-500" },
                    {state_display}
                }
            }
            if let Some((progress, total)) = current_state.flash_progress() {
                ProgressBar { percent: percent_complete(progress, total) }
                span {
                    class: "text-xs text-gray-400",
                    "{progress / BYTES_PER_KB} / {total / BYTES_PER_KB} KB"
                }
            }
        }
    }
}
