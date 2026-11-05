use gpui::{AnyElement, IntoElement as _, ParentElement as _, Styled as _, div};
use odyssey_hub_common::device::Device;
use odyssey_hub_server::firmware::FirmwareManifest;

use super::update_check::{format_version, is_in_bootloader};
use super::update_manager::FirmwareUpdateManager;
use super::update_notes::{HeldBack, NetCoreBlocked, held_back_note, net_core_blocked_note};
use super::update_task::start_firmware_update;
use crate::components::ui::{ButtonKind, button};
use crate::theme::Palette;

pub(super) struct Offer<'a> {
    pub current: [u16; 3],
    pub available: &'a str,
    pub device_type: &'a str,
    pub vid: u16,
    pub pid: u16,
    pub held_back: Option<&'a HeldBack>,
    pub net_core_blocked: Option<&'a NetCoreBlocked>,
}

/// The yellow "update available" line with its notes and the Update button.
pub(super) fn update_available(
    device: &Device,
    manifest: Option<&FirmwareManifest>,
    manager: &FirmwareUpdateManager,
    offer: Offer<'_>,
    p: Palette,
) -> AnyElement {
    let available = offer.available;
    let text = if is_in_bootloader(device) {
        format!("Device in DFU mode — flash v{available}")
    } else {
        format!(
            "Update available: v{} → v{available}",
            format_version(offer.current)
        )
    };
    let mut row = div()
        .flex()
        .items_center()
        .gap_2()
        .child(div().text_xs().text_color(p.warning_ink).child(text));
    if let Some(held_back) = offer.held_back {
        row = row.child(held_back_note(held_back, p));
    }
    if let Some(blocked) = offer.net_core_blocked {
        row = row.child(net_core_blocked_note(blocked, Some(available), p));
    }

    let (manager, manifest, device) = (manager.clone(), manifest.cloned(), device.clone());
    let device_type = offer.device_type.to_owned();
    let (vid, pid) = (offer.vid, offer.pid);
    row.child(
        // The state is refreshed on a timer, so a second click can land before
        // this branch stops being rendered.
        button(
            format!("update-{:02x?}", device.uuid),
            "Update",
            ButtonKind::Primary,
            p,
        )
        .small()
        .disabled(manager.is_updating(&device.uuid))
        .on_click(move |_, _, _| {
            start_firmware_update(
                manager.clone(),
                manifest.clone(),
                device.clone(),
                device_type.clone(),
                vid,
                pid,
            );
        }),
    )
    .into_any_element()
}
