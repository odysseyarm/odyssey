//! The connected/disconnected pill beside an accessory.

use std::f32::consts::PI;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, ElementId, FontWeight, IntoElement, ParentElement as _,
    Styled as _, div,
};

use crate::theme::Palette;

/// A connected accessory pulses its dot, like the stylesheet's `animate-pulse`.
pub(super) fn status_chip(
    id: impl Into<ElementId>,
    connected: bool,
    p: Palette,
) -> impl IntoElement {
    let (label, bg, ink) = if connected {
        ("Connected", p.success_soft, p.success_ink)
    } else {
        ("Disconnected", p.surface_alt, p.fg_muted)
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
            |dot, opacity| dot.opacity(opacity),
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
