use dioxus::prelude::*;

use super::progress_bar::ProgressBar;
use super::update_manager::{FirmwareUpdateManager, use_polled_state};
use super::update_state::{BYTES_PER_KB, FirmwareUpdateState, percent_complete};

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

/// A device whose update is running while it is absent from the device list.
#[component]
pub(crate) fn UpdatingDeviceRow(props: UpdatingDeviceRowProps) -> Element {
    let uuid = props.uuid;

    let current_state = use_polled_state(&props.manager, uuid)();
    let state_display = state_display(&current_state);

    let is_error = matches!(current_state, FirmwareUpdateState::Failed(_));
    let is_complete = matches!(current_state, FirmwareUpdateState::Complete);

    rsx! {
        div {
            class: "flex flex-col p-3 bg-surface-alt rounded-lg gap-2",
            div {
                class: "flex items-center justify-between",
                div {
                    class: "flex items-center gap-3",
                    div {
                        class: if is_error { "size-2 bg-danger rounded-full" }
                               else if is_complete { "size-2 bg-success rounded-full" }
                               else { "size-2 bg-warning rounded-full animate-pulse" }
                    }
                    span {
                        class: "text-sm font-mono text-fg",
                        "{uuid[0]:02x}:{uuid[1]:02x}:{uuid[2]:02x}:{uuid[3]:02x}:{uuid[4]:02x}:{uuid[5]:02x}"
                    }
                }
                span {
                    class: if is_error { "text-xs text-danger-ink" }
                           else if is_complete { "text-xs text-success-ink" }
                           else { "text-xs text-warning-ink" },
                    {state_display}
                }
            }
            if let Some((progress, total)) = current_state.flash_progress() {
                ProgressBar { percent: percent_complete(progress, total) }
                span {
                    class: "text-xs text-fg-muted",
                    "{progress / BYTES_PER_KB} / {total / BYTES_PER_KB} KB"
                }
            }
        }
    }
}
