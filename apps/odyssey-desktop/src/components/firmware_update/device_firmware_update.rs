use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::{Device, DeviceCapabilities, DeviceCapabilities as DC};
use odyssey_hub_server::firmware::{self, FirmwareManifest};
use slab::Slab;

use super::update_check::desktop_version;
use super::update_manager::FirmwareUpdateManager;
use super::update_notes::{HeldBack, NetCoreBlocked, held_back_note, net_core_blocked_note};
use super::update_state::FirmwareUpdateState;
use super::update_task::start_firmware_update;

/// Props for the device firmware update button/indicator
#[derive(Props, Clone, PartialEq)]
pub(crate) struct DeviceFirmwareUpdateProps {
    /// The device to check/update
    pub device: Device,
    /// Cached firmware manifest (shared across all devices)
    pub manifest: Signal<Option<FirmwareManifest>>,
    /// Firmware update manager
    pub manager: FirmwareUpdateManager,
    /// Live device list, for reading the net core release back after it is written
    pub devices: SyncSignal<Slab<Device>>,
}

/// A component that shows firmware update status and allows updating a single device
#[component]
pub(crate) fn DeviceFirmwareUpdate(props: DeviceFirmwareUpdateProps) -> Element {
    let device = props.device.clone();
    let manifest = props.manifest;
    let manager = props.manager.clone();
    let device_uuid = device.uuid;

    // Local state that we poll from the manager
    let mut local_state = use_signal(|| manager.get_state(&device_uuid));

    // Poll the manager for state updates every 100ms
    {
        let manager = manager.clone();
        use_future(move || {
            let manager = manager.clone();
            async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let new_state = manager.get_state(&device_uuid);
                    local_state.set(new_state);
                }
            }
        });
    }

    // Check if update is available - runs on every render when props change
    // We do this synchronously during render rather than in use_effect because
    // use_effect captures values and won't see prop changes
    {
        let current_state = manager.get_state(&device_uuid);

        // Only update state if we're not in the middle of an update
        if matches!(
            current_state,
            FirmwareUpdateState::Idle
                | FirmwareUpdateState::Available { .. }
                | FirmwareUpdateState::NeedsUsbConnection { .. }
                | FirmwareUpdateState::NeedsNewerDesktop(_)
                | FirmwareUpdateState::NetCoreBlockedNoPath(_)
        ) {
            let mut next_state = FirmwareUpdateState::Idle;

            if let Some(manifest) = manifest() {
                let device_version = device.firmware_version;
                let pid = device.product_id;
                // True only when the device is stuck in DFU bootloader (no CONTROL).
                // App-mode devices expose a DFU interface too but are NOT in bootloader mode.
                let is_dfu_device = device.capabilities.contains(DC::DFU)
                    && !device.capabilities.contains(DC::CONTROL);

                // Skip if no product ID or no version (version is always readable via control protocol,
                // so [0,0,0] only occurs for devices that are already in DFU bootloader mode).
                if pid == 0 || device_version == [0, 0, 0] && !is_dfu_device {
                    // No product ID, or version unreadable and no DFU interface — skip
                } else
                // Look up device type from its product ID
                if let Some((vid, pid, device_type)) = firmware::device_info_from_pid(pid) {
                    // DFU-bootloader devices (no CONTROL, version [0,0,0]) — show latest firmware.
                    // Connected devices — show latest firmware only if it is actually newer,
                    // allowing cross-major/minor updates (e.g. protocol upgrades).
                    // The same net core reading feeds `run_firmware_update`, so the
                    // entry shown here is the entry that gets installed.
                    let offer = firmware::select_firmware(
                        &manifest,
                        device_type,
                        desktop_version(),
                        device.netcore_release,
                    );
                    // A zero the hub has not settled on yet can mean the net core has
                    // not answered (e.g. right after a transport swap), so it is not
                    // judged as "behind".
                    let readable_netcore = firmware::known_netcore_release(
                        device
                            .capabilities
                            .contains(DeviceCapabilities::NETCORE_KNOWN),
                        device.netcore_release,
                    );
                    let is_newer = |fw: &firmware::FirmwareEntry| {
                        is_dfu_device
                            || firmware::is_worth_installing(fw, device_version, readable_netcore)
                    };
                    let maybe_fw = offer.offered.filter(|fw| is_newer(fw));
                    let held_back = offer
                        .held_back
                        .filter(|fw| is_newer(fw))
                        .map(|fw| HeldBack {
                            version: fw.version.clone(),
                            min_desktop: fw.min_desktop.clone(),
                        });
                    let net_core_blocked =
                        offer
                            .net_core_blocked
                            .filter(|fw| is_newer(fw))
                            .map(|fw| NetCoreBlocked {
                                version: fw.version.clone(),
                            });

                    if let Some(available_fw) = maybe_fw {
                        let available_version = available_fw.version.clone();
                        let device_type = device_type.to_string();

                        // DFU devices always show the update button (they need to be flashed to
                        // recover).  Normal devices need CONTROL capability for USB flashing.
                        if is_dfu_device
                            || device.capabilities.contains(DeviceCapabilities::CONTROL)
                        {
                            let new_state = FirmwareUpdateState::Available {
                                current: device_version,
                                available: available_version.clone(),
                                device_type,
                                vid,
                                pid,
                                held_back: held_back.clone(),
                                net_core_blocked: net_core_blocked.clone(),
                            };
                            if !matches!(current_state, FirmwareUpdateState::Available { .. }) {
                                if is_dfu_device {
                                    tracing::info!(
                                        "DFU device {:02x?} (PID 0x{:04x}) ready to flash -> {}",
                                        device_uuid,
                                        pid,
                                        available_version
                                    );
                                } else {
                                    tracing::info!(
                                        "Update available for {:02x?}: {}.{}.{} -> {}",
                                        device_uuid,
                                        device_version[0],
                                        device_version[1],
                                        device_version[2],
                                        available_version
                                    );
                                }
                            }
                            next_state = new_state;
                        } else {
                            // Device doesn't have USB control - prompt user to plug in USB
                            let new_state = FirmwareUpdateState::NeedsUsbConnection {
                                current: device_version,
                                available: available_version.clone(),
                            };
                            if !matches!(
                                current_state,
                                FirmwareUpdateState::NeedsUsbConnection { .. }
                            ) {
                                tracing::info!(
                                    "Update available for {:02x?}: {}.{}.{} -> {} (needs USB connection)",
                                    device_uuid,
                                    device_version[0],
                                    device_version[1],
                                    device_version[2],
                                    available_version
                                );
                            }
                            next_state = new_state;
                        }
                    } else if let Some(held_back) = held_back {
                        next_state = FirmwareUpdateState::NeedsNewerDesktop(held_back);
                    } else if let Some(blocked) = net_core_blocked {
                        next_state = FirmwareUpdateState::NetCoreBlockedNoPath(blocked);
                    }
                }
            }

            manager.set_state(device_uuid, next_state);
        }
    }

    let state = local_state();

    match state {
        FirmwareUpdateState::Idle => rsx! {},
        FirmwareUpdateState::NeedsUsbConnection { current, available } => {
            rsx! {
                div {
                    class: "flex items-center gap-2",
                    span {
                        class: "text-xs text-yellow-600 dark:text-yellow-400",
                        "Update available: v{current[0]}.{current[1]}.{current[2]} → v{available}"
                    }
                    span {
                        class: "text-xs text-gray-500 dark:text-gray-400 italic",
                        "Plug in USB to update"
                    }
                }
            }
        }
        FirmwareUpdateState::NeedsNewerDesktop(held_back) => held_back_note(&held_back),
        FirmwareUpdateState::NetCoreBlockedNoPath(blocked) => net_core_blocked_note(&blocked, None),
        FirmwareUpdateState::Available {
            current,
            available,
            device_type,
            vid,
            pid,
            held_back,
            net_core_blocked,
        } => {
            let manager = props.manager.clone();
            let devices = props.devices;
            let manifest = manifest;
            let device = device.clone();
            let is_dfu_bootloader =
                device.capabilities.contains(DC::DFU) && !device.capabilities.contains(DC::CONTROL);

            rsx! {
                div {
                    class: "flex items-center gap-2",
                    span {
                        class: "text-xs text-yellow-600 dark:text-yellow-400",
                        if is_dfu_bootloader { "Device in DFU mode — flash v{available}" }
                        else { "Update available: v{current[0]}.{current[1]}.{current[2]} → v{available}" }
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
                            let manager = manager.clone();
                            let manifest = manifest.clone();
                            let device = device.clone();
                            let device_type = device_type.clone();

                            // Start the update - runs entirely on tokio
                            start_firmware_update(
                                manager,
                                manifest(),
                                device,
                                device_type,
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
        FirmwareUpdateState::Downloading => rsx! {
            span { class: "text-xs text-blue-500", "Downloading..." }
        },
        FirmwareUpdateState::Detaching => rsx! {
            span { class: "text-xs text-blue-500", "Preparing device..." }
        },
        FirmwareUpdateState::WaitingForDfu => rsx! {
            span { class: "text-xs text-blue-500", "Waiting for DFU mode..." }
        },
        FirmwareUpdateState::Flashing { progress, total } => {
            let percent = if total > 0 {
                (progress * 100) / total
            } else {
                0
            };
            rsx! {
                div {
                    class: "flex flex-col gap-1 min-w-32",
                    span { class: "text-xs text-blue-500", "Flashing... {percent}%" }
                    div {
                        class: "w-full bg-gray-200 dark:bg-gray-700 rounded-full h-2",
                        div {
                            class: "bg-blue-600 h-2 rounded-full transition-all duration-100",
                            style: "width: {percent}%",
                        }
                    }
                    span { class: "text-xs text-gray-400", "{progress / 1024} / {total / 1024} KB" }
                }
            }
        }
        FirmwareUpdateState::FlashingNetCore { progress, total } => {
            let percent = if total > 0 {
                (progress * 100) / total
            } else {
                0
            };
            rsx! {
                div {
                    class: "flex flex-col gap-1 min-w-32",
                    span { class: "text-xs text-blue-500", "Updating radio firmware... {percent}%" }
                    div {
                        class: "w-full bg-gray-200 dark:bg-gray-700 rounded-full h-2",
                        div {
                            class: "bg-blue-600 h-2 rounded-full transition-all duration-100",
                            style: "width: {percent}%",
                        }
                    }
                    span { class: "text-xs text-gray-400", "{progress / 1024} / {total / 1024} KB" }
                }
            }
        }
        FirmwareUpdateState::VerifyingNetCore => rsx! {
            span { class: "text-xs text-blue-500", "Checking radio firmware..." }
        },
        FirmwareUpdateState::InstallingNetCore => rsx! {
            span {
                class: "text-xs text-blue-500",
                "Installing radio firmware..."
            }
        },
        FirmwareUpdateState::Rebooting => rsx! {
            span { class: "text-xs text-blue-500", "Rebooting device..." }
        },
        FirmwareUpdateState::Verifying => rsx! {
            span { class: "text-xs text-blue-500", "Verifying update..." }
        },
        FirmwareUpdateState::Complete => rsx! {
            span { class: "text-xs text-green-500", "Update complete!" }
        },
        FirmwareUpdateState::Failed(ref error) => rsx! {
            div {
                class: "flex flex-col gap-1",
                span { class: "text-xs text-red-500 font-semibold", "Update failed" }
                span { class: "text-xs text-red-400", "{error}" }
            }
        },
    }
}
