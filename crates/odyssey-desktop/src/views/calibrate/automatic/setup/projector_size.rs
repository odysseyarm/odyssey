//! The size question for a projector: optional, as a diagonal or a width.

use gpui::{App, Div, Entity, FontWeight, ParentElement as _, Styled as _, div};

use super::size_box::SizeBox;
use crate::theme::Palette;
use crate::views::calibrate::automatic::settings::SizeField;
use crate::views::calibrate::automatic::ui::{act_boxed, segmented};
use crate::views::calibrate::automatic::wizard::Wizard;

pub(super) fn projector_size(
    p: &Palette,
    wizard: &Entity<Wizard>,
    size_box: &Entity<SizeBox>,
    cx: &App,
) -> Div {
    let field = wizard.read(cx).field();
    let options = [
        (SizeField::Diagonal, "Diagonal"),
        (SizeField::Width, "Width"),
    ]
    .into_iter()
    .map(|(option, label)| {
        (
            label.into(),
            option == field,
            act_boxed(wizard, move |w, cx| {
                if w.size_field != option {
                    w.size_text.clear();
                    w.size_field = option;
                    cx.notify();
                }
            }),
        )
    })
    .collect();
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.fg_strong)
                .child("We'll measure the picture with the camera."),
        )
        .child(div().text_base().text_color(p.fg_muted).child(
            "If you happen to know how big the picture is, type it in for a slightly more exact result. Otherwise leave it empty.",
        ))
        .child(segmented(p, "projector-field", options, 16.0))
        .child(size_box.clone())
}
