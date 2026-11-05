//! The slot a finished calibration is saved under.

use gpui::{
    App, Div, Entity, FontWeight, InteractiveElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, px,
};
use odyssey_common::MAX_SCREEN_ID;

use crate::theme::{Palette, RADIUS};
use crate::views::calibrate::automatic::screens::screen_label;
use crate::views::calibrate::automatic::wizard::Wizard;

pub(super) fn screen_number_picker(p: Palette, wizard: &Entity<Wizard>, cx: &App) -> Div {
    let state = wizard.read(cx);
    let chosen = state.screen_id();
    let saved = state.saved;
    let mut grid = div().grid().grid_cols(6).gap_2();
    for id in 0..=MAX_SCREEN_ID {
        let selected = id == chosen;
        let taken = saved[usize::from(id)];
        let (bg, border, fg, sub) = if selected {
            (p.accent, p.accent, p.on_accent, p.on_accent)
        } else {
            (p.surface, p.line, p.fg, p.fg_muted)
        };
        let wizard = wizard.clone();
        let mut cell = div()
            .id(("screen-number", usize::from(id)))
            .flex()
            .flex_col()
            .items_center()
            .px_2()
            .py_2()
            .rounded(px(RADIUS))
            .border_2()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .cursor_pointer()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(screen_label(id)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(sub)
                    .child(if taken { "Calibrated" } else { "Empty" }),
            )
            .on_click(move |_, _, cx| {
                wizard.update(cx, |w, cx| {
                    w.screen_choice = Some(id);
                    cx.notify();
                });
            });
        if !selected {
            cell = cell.hover(move |s| s.border_color(p.accent_ink));
        }
        grid = grid.child(cell);
    }
    let mut col = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.fg_muted)
                .child("Save the result as:"),
        )
        .child(grid);
    if saved[usize::from(chosen)] {
        col = col.child(div().text_sm().text_color(p.warning_ink).child(format!(
            "{} already has a calibration. You'll be asked before it is replaced.",
            screen_label(chosen)
        )));
    }
    col
}
