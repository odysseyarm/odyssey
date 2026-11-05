//! The notice shown when offline mode is on or the policy file has problems.

use gpui::{AnyElement, IntoElement as _, ParentElement as _, Styled as _, div};

use crate::components::ui::{self, AlertKind};
use crate::theme::Palette;

/// Says so when offline mode is on and where firmware updates come from, and
/// shows what is wrong with the firmware pack or the policy file. Shows nothing
/// when everything is online and fine.
pub(super) fn offline_notice(error: Option<&str>, p: Palette) -> Option<AnyElement> {
    let policy = odyssey_hub_server::policy::current();
    if !policy.offline && policy.issues.is_empty() {
        return None;
    }
    let dir = odyssey_hub_server::policy::firmware_pack_dir()
        .display()
        .to_string();

    let mut body = div().flex().flex_col().gap_1();
    if policy.offline {
        body = body
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Offline mode is on"),
            )
            .child(format!(
                "Odyssey won't connect to the internet. Firmware updates come from the firmware pack in {dir}."
            ));
        if let Some(error) = error {
            body = body.child(
                div()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(error.to_owned()),
            );
        }
    }
    for issue in &policy.issues {
        body = body.child(
            div()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(issue.clone()),
        );
    }
    Some(ui::alert(AlertKind::Warning, p, body).into_any_element())
}
