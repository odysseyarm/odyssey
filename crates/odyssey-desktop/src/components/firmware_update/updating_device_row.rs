use gpui::{AnyElement, IntoElement as _, ParentElement as _, Styled as _, div, px};

use super::update_manager::FirmwareUpdateManager;
use super::update_state::{BYTES_PER_KB, FirmwareUpdateState, percent_complete};
use crate::components::ui;
use crate::theme::{Palette, RADIUS};

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
pub(crate) fn updating_device_row(
    uuid: [u8; 6],
    manager: &FirmwareUpdateManager,
    p: Palette,
) -> AnyElement {
    let state = manager.get_state(&uuid);
    let is_error = matches!(state, FirmwareUpdateState::Failed(_));
    let is_complete = matches!(state, FirmwareUpdateState::Complete);
    let (dot, ink) = if is_error {
        (p.danger, p.danger_ink)
    } else if is_complete {
        (p.success, p.success_ink)
    } else {
        (p.warning, p.warning_ink)
    };

    let mut row = div()
        .flex()
        .flex_col()
        .p_3()
        .gap_2()
        .rounded(px(RADIUS))
        .bg(p.surface_alt)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().size_2().rounded_full().bg(dot))
                        .child(
                            div()
                                .text_sm()
                                .font_family("monospace")
                                .text_color(p.fg)
                                .child(format!(
                                    "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                                    uuid[0], uuid[1], uuid[2], uuid[3], uuid[4], uuid[5]
                                )),
                        ),
                )
                .child(div().text_xs().text_color(ink).child(state_display(&state))),
        );
    if let Some((progress, total)) = state.flash_progress() {
        row = row.child(ui::progress_bar(p, percent_complete(progress, total))).child(
            div().text_xs().text_color(p.fg_muted).child(format!(
                "{} / {} KB",
                progress / BYTES_PER_KB,
                total / BYTES_PER_KB
            )),
        );
    }
    row.into_any_element()
}
