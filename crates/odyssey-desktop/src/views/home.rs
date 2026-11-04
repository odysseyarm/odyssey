//! The landing page: connected devices with their firmware and settings, and
//! the screen to calibrate or zero on.

mod connected_device_row;
mod connected_devices;
mod firmware_state;
mod offline_notice;
mod screen_actions;

use connected_devices::ConnectedDevices;
use dioxus::prelude::*;
use firmware_state::{use_firmware_manifest, use_updating_uuids};
use offline_notice::OfflineNotice;
use screen_actions::ScreenActions;

use crate::components::{FirmwareUpdateManager, Page};
use crate::hub::HubContext;

#[component]
pub fn Home() -> Element {
    let hub = use_context::<Signal<HubContext>>();

    // Shared by the tokio update tasks and the UI.
    let manager = use_context::<FirmwareUpdateManager>();
    let firmware = use_firmware_manifest();
    let updating = use_updating_uuids(&manager)();

    rsx! {
        Page {
            title: "Home",
            OfflineNotice { error: (firmware.error)() }
            ConnectedDevices { hub, manifest: firmware.manifest, manager, updating }
            ScreenActions { hub }
        }
    }
}
