use dioxus::prelude::*;
use odyssey_hub_common::device::Device;
use odyssey_hub_server::firmware::FirmwareManifest;
use slab::Slab;

use super::update_check::{format_version, is_in_bootloader};
use super::update_manager::FirmwareUpdateManager;
use super::update_notes::{HeldBack, NetCoreBlocked, held_back_note, net_core_blocked_note};
use super::update_task::start_firmware_update;

/// The yellow "update available" line with its notes and the Update button.
#[component]
pub(super) fn UpdateAvailable(
    device: Device,
    manifest: Signal<Option<FirmwareManifest>>,
    manager: FirmwareUpdateManager,
    devices: SyncSignal<Slab<Device>>,
    current: [u16; 3],
    available: String,
    device_type: String,
    vid: u16,
    pid: u16,
    held_back: Option<HeldBack>,
    net_core_blocked: Option<NetCoreBlocked>,
) -> Element {
    let is_dfu_bootloader = is_in_bootloader(&device);
    let current = format_version(current);

    rsx! {
        div {
            class: "flex items-center gap-2",
            span {
                class: "text-xs text-yellow-600 dark:text-yellow-400",
                if is_dfu_bootloader { "Device in DFU mode — flash v{available}" }
                else { "Update available: v{current} → v{available}" }
            }
            if let Some(held_back) = &held_back {
                {held_back_note(held_back)}
            }
            if let Some(blocked) = &net_core_blocked {
                {net_core_blocked_note(blocked, Some(&available))}
            }
            button {
                // The state below is polled, so a second click can land
                // before this branch stops being rendered.
                disabled: manager.is_updating(&device.uuid),
                class: "btn-primary-sm",
                onclick: move |_| {
                    // Start the update - runs entirely on tokio
                    start_firmware_update(
                        manager.clone(),
                        manifest(),
                        device.clone(),
                        device_type.clone(),
                        vid,
                        pid,
                        devices,
                    );
                },
                "Update"
            }
        }
    }
}
