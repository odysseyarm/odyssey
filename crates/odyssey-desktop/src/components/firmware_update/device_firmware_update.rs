use dioxus::prelude::*;
use odyssey_hub_common::device::Device;
use odyssey_hub_server::firmware::FirmwareManifest;
use slab::Slab;

use super::progress_bar::ProgressBar;
use super::update_available::UpdateAvailable;
use super::update_check::{check_for_update, format_version};
use super::update_manager::{FirmwareUpdateManager, use_polled_state};
use super::update_notes::{held_back_note, net_core_blocked_note};
use super::update_state::{BYTES_PER_KB, FirmwareUpdateState, percent_complete};

#[derive(Props, Clone, PartialEq)]
pub(crate) struct DeviceFirmwareUpdateProps {
    pub device: Device,
    /// Shared by every device row.
    pub manifest: Signal<Option<FirmwareManifest>>,
    pub manager: FirmwareUpdateManager,
    /// Live device list, for reading the net core release back after it is written
    pub devices: SyncSignal<Slab<Device>>,
}

/// Firmware update status for one device, with the button that starts the update.
#[component]
pub(crate) fn DeviceFirmwareUpdate(props: DeviceFirmwareUpdateProps) -> Element {
    let DeviceFirmwareUpdateProps {
        device,
        manifest,
        manager,
        devices,
    } = props;
    let device_uuid = device.uuid;

    let local_state = use_polled_state(&manager, device_uuid);

    // Checked during render rather than in `use_effect`: an effect captures its
    // values and would not see prop changes.
    let current_state = manager.get_state(&device_uuid);
    // A running update owns the state; availability must not overwrite it.
    if current_state.is_before_update() {
        let next_state = match manifest() {
            Some(manifest) => check_for_update(&manifest, &device, &current_state),
            None => FirmwareUpdateState::Idle,
        };
        manager.set_before_update_state(device_uuid, next_state);
    }

    let state = local_state();
    match &state {
        FirmwareUpdateState::Idle => rsx! {},
        FirmwareUpdateState::NeedsUsbConnection { current, available } => {
            let current = format_version(*current);
            rsx! {
                div {
                    class: "flex items-center gap-2",
                    span {
                        class: "text-xs text-yellow-600 dark:text-yellow-400",
                        "Update available: v{current} → v{available}"
                    }
                    span {
                        class: "text-xs text-gray-500 dark:text-gray-400 italic",
                        "Plug in USB to update"
                    }
                }
            }
        }
        FirmwareUpdateState::NeedsNewerDesktop(held_back) => held_back_note(held_back),
        FirmwareUpdateState::NetCoreBlockedNoPath(blocked) => net_core_blocked_note(blocked, None),
        FirmwareUpdateState::Available {
            current,
            available,
            device_type,
            vid,
            pid,
            held_back,
            net_core_blocked,
        } => rsx! {
            UpdateAvailable {
                device,
                manifest,
                manager,
                devices,
                current: *current,
                available: available.clone(),
                device_type: device_type.clone(),
                vid: *vid,
                pid: *pid,
                held_back: held_back.clone(),
                net_core_blocked: net_core_blocked.clone(),
            }
        },
        FirmwareUpdateState::Flashing { progress, total }
        | FirmwareUpdateState::FlashingNetCore { progress, total } => rsx! {
            FlashProgress {
                label: state.status_text(),
                progress: *progress,
                total: *total,
            }
        },
        FirmwareUpdateState::Downloading
        | FirmwareUpdateState::Detaching
        | FirmwareUpdateState::WaitingForDfu
        | FirmwareUpdateState::VerifyingNetCore
        | FirmwareUpdateState::InstallingNetCore
        | FirmwareUpdateState::Rebooting
        | FirmwareUpdateState::Verifying => rsx! {
            span { class: "text-xs text-blue-500", {state.status_text()} }
        },
        FirmwareUpdateState::Complete => rsx! {
            span { class: "text-xs text-green-500", {state.status_text()} }
        },
        FirmwareUpdateState::Failed(error) => rsx! {
            div {
                class: "flex flex-col gap-1",
                span { class: "text-xs text-red-500 font-semibold", {state.status_text()} }
                span { class: "text-xs text-red-400", {error.clone()} }
            }
        },
    }
}

/// A label, a progress bar and the byte counts of an image being written.
#[component]
fn FlashProgress(label: &'static str, progress: usize, total: usize) -> Element {
    let percent = percent_complete(progress, total);
    rsx! {
        div {
            class: "flex flex-col gap-1 min-w-32",
            span { class: "text-xs text-blue-500", "{label} {percent}%" }
            ProgressBar { percent }
            span {
                class: "text-xs text-gray-400",
                "{progress / BYTES_PER_KB} / {total / BYTES_PER_KB} KB"
            }
        }
    }
}
