use gpui::{AnyElement, IntoElement as _, ParentElement as _, Styled as _, div};
use odyssey_hub_common::device::Device;
use odyssey_hub_server::firmware::FirmwareManifest;

use super::update_available::{Offer, update_available};
use super::update_check::{check_for_update, format_version};
use super::update_manager::FirmwareUpdateManager;
use super::update_notes::{held_back_note, net_core_blocked_note};
use super::update_state::{BYTES_PER_KB, FirmwareUpdateState, percent_complete};
use crate::components::ui;
use crate::theme::Palette;

/// Firmware update status for one device, with the button that starts the update.
///
/// Availability is computed on every render rather than cached: the manifest
/// and the device can change between renders, and a running update owns the
/// state, which `set_before_update_state` never overwrites.
pub(crate) fn device_firmware_update(
    device: &Device,
    manifest: Option<&FirmwareManifest>,
    manager: &FirmwareUpdateManager,
    p: Palette,
    expanded: bool,
) -> AnyElement {
    let device_uuid = device.uuid;
    let current_state = manager.get_state(&device_uuid);
    if current_state.is_before_update() {
        let next_state = match manifest {
            Some(manifest) => check_for_update(manifest, device, &current_state),
            None => FirmwareUpdateState::Idle,
        };
        manager.set_before_update_state(device_uuid, next_state);
    }

    let state = manager.get_state(&device_uuid);
    if !expanded
        && matches!(state, FirmwareUpdateState::Available { .. })
        && device
            .capabilities
            .contains(odyssey_hub_common::device::DeviceCapabilities::CONTROL)
    {
        return div()
            .text_xs()
            .text_color(p.accent_ink)
            .child("Firmware update available — open Advanced device details to manage.")
            .into_any_element();
    }
    match &state {
        FirmwareUpdateState::Idle => div().into_any_element(),
        FirmwareUpdateState::NeedsUsbConnection { current, available } => div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().text_xs().text_color(p.warning_ink).child(format!(
                "Update available: v{} → v{available}",
                format_version(*current)
            )))
            .child(
                div()
                    .text_xs()
                    .italic()
                    .text_color(p.fg_muted)
                    .child("Plug in USB to update"),
            )
            .into_any_element(),
        FirmwareUpdateState::NeedsNewerDesktop(held_back) => held_back_note(held_back, p),
        FirmwareUpdateState::NetCoreBlockedNoPath(blocked) => {
            net_core_blocked_note(blocked, None, p)
        }
        FirmwareUpdateState::Available {
            current,
            available,
            device_type,
            vid,
            pid,
            held_back,
            net_core_blocked,
        } => update_available(
            device,
            manifest,
            manager,
            Offer {
                current: *current,
                available,
                device_type,
                vid: *vid,
                pid: *pid,
                held_back: held_back.as_ref(),
                net_core_blocked: net_core_blocked.as_ref(),
            },
            p,
        ),
        FirmwareUpdateState::Flashing { progress, total }
        | FirmwareUpdateState::FlashingNetCore { progress, total } => {
            flash_progress(state.status_text(), *progress, *total, p)
        }
        FirmwareUpdateState::Downloading
        | FirmwareUpdateState::Detaching
        | FirmwareUpdateState::WaitingForDfu
        | FirmwareUpdateState::VerifyingNetCore
        | FirmwareUpdateState::InstallingNetCore
        | FirmwareUpdateState::Rebooting
        | FirmwareUpdateState::Verifying => div()
            .text_xs()
            .text_color(p.accent_ink)
            .child(state.status_text())
            .into_any_element(),
        FirmwareUpdateState::Complete => {
            let manager = manager.clone();
            div()
                .text_xs()
                .text_color(p.success_ink)
                .child(state.status_text())
                .child(
                    ui::button(
                        gpui::SharedString::from(format!(
                            "ack-firmware-{}",
                            hex::encode(device_uuid)
                        )),
                        "Done",
                        ui::ButtonKind::Secondary,
                        p,
                    )
                    .small()
                    .on_click(move |_, _, _| manager.dismiss_result(device_uuid)),
                )
                .into_any_element()
        }
        FirmwareUpdateState::Failed(error) => {
            let manager = manager.clone();
            div()
                .flex()
                .flex_col()
                .gap_1()
                .text_xs()
                .text_color(p.danger_ink)
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(state.status_text()))
                .child(error.clone())
                .child("Reconnect USB and check power. If the tracker remains in recovery mode, retry the firmware update.")
                .child(ui::button(
                    gpui::SharedString::from(format!("retry-firmware-{}", hex::encode(device_uuid))),
                    "Check update again",
                    ui::ButtonKind::Secondary,
                    p,
                ).small().on_click(move |_, _, _| manager.retry_failed(device_uuid)))
                .into_any_element()
        }
    }
}

/// A label, a progress bar and the byte counts of an image being written.
pub(super) fn flash_progress(label: &str, progress: usize, total: usize, p: Palette) -> AnyElement {
    let percent = percent_complete(progress, total);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w(gpui::px(128.0))
        .child(
            div()
                .text_xs()
                .text_color(p.accent_ink)
                .child(format!("{label} {percent}%")),
        )
        .child(ui::progress_bar(p, percent))
        .child(div().text_xs().text_color(p.fg_muted).child(format!(
            "{} / {} KB",
            progress / BYTES_PER_KB,
            total / BYTES_PER_KB
        )))
        .into_any_element()
}
