//! Accessory lifecycle status, including intentional idle marker links.

use std::f32::consts::PI;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, ElementId, FontWeight, IntoElement, ParentElement as _, Styled,
    div,
};
use odyssey_hub_common::accessory::AccessoryConnectionState;

use crate::theme::Palette;

/// A connected accessory pulses its dot, like the stylesheet's `animate-pulse`.
pub(super) fn status_chip(
    id: impl Into<ElementId>,
    state: &AccessoryConnectionState,
    p: Palette,
) -> impl IntoElement {
    let connected = *state == AccessoryConnectionState::Connected;
    let (label, bg, ink) = match state {
        AccessoryConnectionState::Idle => ("Idle", p.surface_alt, p.fg_muted),
        AccessoryConnectionState::Connecting => ("Connecting", p.info_soft, p.info_ink),
        AccessoryConnectionState::Connected => ("Connected", p.success_soft, p.success_ink),
        AccessoryConnectionState::Failed => ("Connection failed", p.danger_soft, p.danger),
        AccessoryConnectionState::RadioUnavailable => {
            ("Radio unavailable", p.warning_soft, p.warning_ink)
        }
    };
    let dot = div()
        .size_2()
        .rounded_full()
        .bg(if connected { p.success } else { p.fg_faint });
    let dot = if connected {
        dot.with_animation(
            id,
            Animation::new(Duration::from_secs(2))
                .repeat()
                .with_easing(|t| 1.0 - 0.5 * (t * PI).sin()),
            Styled::opacity,
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    };
    div()
        .flex()
        .items_center()
        .gap_2()
        .rounded_full()
        .px_2()
        .py_0p5()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .bg(bg)
        .text_color(ink)
        .child(dot)
        .child(label)
}
