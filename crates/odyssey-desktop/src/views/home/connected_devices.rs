//! The list of devices the hub reports, plus any being updated that it does not.

use dioxus::prelude::*;
use dioxus_router::Link;
use odyssey_hub_server::firmware::FirmwareManifest;

use super::connected_device_row::ConnectedDeviceRow;
use crate::Route;
use crate::components::{FirmwareUpdateManager, UpdatingDeviceRow};
use crate::hub::HubContext;

#[component]
pub(super) fn ConnectedDevices(
    hub: Signal<HubContext>,
    manifest: Signal<Option<FirmwareManifest>>,
    manager: FirmwareUpdateManager,
    updating: Vec<[u8; 6]>,
) -> Element {
    let devices = (hub().devices)();
    let battery_states = (hub().battery_states)();
    if devices.is_empty() && updating.is_empty() {
        return rsx! {
            div {
                class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow",
                h2 {
                    class: "text-lg font-semibold text-gray-900 dark:text-white mb-1",
                    "No devices connected"
                }
                p {
                    class: "text-sm text-gray-600 dark:text-gray-300",
                    "Plug a device in with its USB cable, or "
                    Link {
                        to: Route::Pairing {},
                        class: "font-medium text-blue-700 underline hover:no-underline dark:text-blue-400",
                        "pair it with a dongle"
                    }
                    "."
                }
            }
        };
    }

    let updating_not_in_list: Vec<[u8; 6]> = updating
        .iter()
        .filter(|uuid| !devices.iter().any(|(_, device)| &device.uuid == *uuid))
        .copied()
        .collect();

    rsx! {
        div {
            class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow",
            h2 {
                class: "text-lg font-semibold text-gray-900 dark:text-white mb-3",
                "Connected Devices"
            }
            div {
                class: "space-y-2",
                for (_slot, device) in devices.iter() {
                    ConnectedDeviceRow {
                        key: "{device.uuid:02x?}",
                        hub,
                        device: device.clone(),
                        is_updating: updating.contains(&device.uuid),
                        battery: battery_states.get(&device.uuid).copied(),
                        manifest,
                        manager: manager.clone(),
                    }
                }
                for uuid in updating_not_in_list {
                    UpdatingDeviceRow {
                        key: "{uuid:02x?}",
                        uuid,
                        manager: manager.clone(),
                    }
                }
            }
        }
    }
}
