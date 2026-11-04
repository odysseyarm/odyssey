use dioxus::prelude::*;

use super::update_manager::FirmwareUpdateManager;
use super::update_state::FirmwareUpdateState;

/// Props for the updating device row component
#[derive(Props, Clone, PartialEq)]
pub(crate) struct UpdatingDeviceRowProps {
    pub uuid: [u8; 6],
    pub manager: FirmwareUpdateManager,
}

/// A component that displays a device being updated (when not in device list)
#[component]
pub(crate) fn UpdatingDeviceRow(props: UpdatingDeviceRowProps) -> Element {
    let uuid = props.uuid;
    let manager = props.manager.clone();

    // Poll state from manager
    let mut state = use_signal(|| manager.get_state(&uuid));

    use_future(move || {
        let manager = manager.clone();
        async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                state.set(manager.get_state(&uuid));
            }
        }
    });

    let current_state = state();

    let state_display = match &current_state {
        FirmwareUpdateState::Idle => "Idle".to_string(),
        FirmwareUpdateState::NeedsUsbConnection { .. } => "Plug in USB to update".to_string(),
        FirmwareUpdateState::NeedsNewerDesktop(_) => "Needs a newer Odyssey Desktop".to_string(),
        FirmwareUpdateState::NetCoreBlockedNoPath(_) => {
            "No update path from this net core".to_string()
        }
        FirmwareUpdateState::Available { .. } => "Update available".to_string(),
        FirmwareUpdateState::Downloading => "Downloading...".to_string(),
        FirmwareUpdateState::Detaching => "Preparing device...".to_string(),
        FirmwareUpdateState::WaitingForDfu => "Waiting for DFU mode...".to_string(),
        FirmwareUpdateState::Flashing { progress, total } => {
            let percent = if *total > 0 {
                (*progress * 100) / *total
            } else {
                0
            };
            format!("Flashing... {percent}%")
        }
        FirmwareUpdateState::FlashingNetCore { progress, total } => {
            let percent = if *total > 0 {
                (*progress * 100) / *total
            } else {
                0
            };
            format!("Updating radio firmware... {percent}%")
        }
        FirmwareUpdateState::VerifyingNetCore => "Checking radio firmware...".to_string(),
        FirmwareUpdateState::InstallingNetCore => "Installing radio firmware...".to_string(),
        FirmwareUpdateState::Rebooting => "Rebooting device...".to_string(),
        FirmwareUpdateState::Verifying => "Verifying update...".to_string(),
        FirmwareUpdateState::Complete => "Update complete!".to_string(),
        FirmwareUpdateState::Failed(e) => format!("Failed: {e}"),
    };

    let is_error = matches!(current_state, FirmwareUpdateState::Failed(_));
    let is_complete = matches!(current_state, FirmwareUpdateState::Complete);
    let is_flashing = matches!(
        current_state,
        FirmwareUpdateState::Flashing { .. } | FirmwareUpdateState::FlashingNetCore { .. }
    );

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
                    "{state_display}"
                }
            }
            if is_flashing {
                {
                    let (progress, total) = match &current_state {
                        FirmwareUpdateState::Flashing { progress, total }
                        | FirmwareUpdateState::FlashingNetCore { progress, total } => (*progress, *total),
                        _ => (0, 1),
                    };
                    let percent = if total > 0 { (progress * 100) / total } else { 0 };
                    rsx! {
                        div {
                            class: "w-full bg-gray-200 dark:bg-gray-700 rounded-full h-2",
                            div {
                                class: "bg-blue-600 h-2 rounded-full transition-all duration-100",
                                style: "width: {percent}%",
                            }
                        }
                        span {
                            class: "text-xs text-gray-400",
                            "{progress / 1024} / {total / 1024} KB"
                        }
                    }
                }
            }
        }
    }
}
