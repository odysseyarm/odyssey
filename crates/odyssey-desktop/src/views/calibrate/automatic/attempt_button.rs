//! "Save details for support": saves the latest calibration attempt, on the failure
//! screen and the review, as one zip a person can send (see `attempt_bundle`).

use gpui::{Div, Entity, ParentElement as _, Styled as _, div};

use super::ui::act;
use super::wizard::Wizard;
use crate::components::ui::{ButtonKind, button};
use crate::theme::Palette;

/// A size for people: "46.3 MB".
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

pub(super) fn save_attempt(p: Palette, wizard: &Entity<Wizard>, state: &Wizard) -> Div {
    let saving = state.attempt_saving;
    let label = if saving {
        "Saving\u{2026}"
    } else {
        "Save details for support\u{2026}"
    };
    let status = match &state.attempt_status {
        Some(Ok(saved)) => Some(div().text_sm().text_color(p.fg).child(format!(
            "Saved {} ({}).",
            saved.path.display(),
            megabytes(saved.bytes)
        ))),
        Some(Err(error)) => Some(
            div()
                .text_sm()
                .text_color(p.danger_ink)
                .child(format!("Couldn't save the details: {error}")),
        ),
        None => None,
    };
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div().flex().flex_wrap().items_center().gap_3().child(
                button("save-attempt", label, ButtonKind::Secondary, p)
                    .disabled(saving)
                    .on_click(act(wizard, |wizard, cx| wizard.save_attempt(cx))),
            ),
        )
        .child(
            div().text_sm().text_color(p.fg_muted).child(
                "A zip of what this attempt used: the frames of the screen measurement (color, depth and infrared) and the infrared frames of the marker measurement. They show the room. It also holds the calibration lines of the app's log, the camera's serial number and the settings, and is tens of megabytes. Nothing is sent anywhere; send it yourself if you want help.",
            ),
        )
        .children(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_in_megabytes() {
        assert_eq!(megabytes(46_300_000), "46.3 MB");
        assert_eq!(megabytes(0), "0.0 MB");
    }
}
