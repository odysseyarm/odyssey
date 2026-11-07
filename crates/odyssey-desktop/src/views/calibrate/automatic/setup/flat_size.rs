//! The size question for a flat screen, answered by the display, the tech or
//! the camera.

use gpui::component::checkbox::Checkbox;
use gpui::{
    AnyElement, App, Div, Entity, FontWeight, IntoElement as _, ParentElement as _, Styled as _,
    div,
};

use super::size_box::SizeBox;
use crate::components::wizard::{Icon, icon};
use crate::theme::Palette;
use crate::views::calibrate::automatic::kind::Finding;
use crate::views::calibrate::automatic::ui::{
    act, info_note, link_button, plain_details, working_row,
};
use crate::views::calibrate::automatic::wizard::Wizard;

pub(super) fn flat_size(
    p: Palette,
    wizard: &Entity<Wizard>,
    size_box: &Entity<SizeBox>,
    details_open: &Entity<bool>,
    cx: &App,
) -> Div {
    let state = wizard.read(cx);
    let Some(finding) = state.finding.clone() else {
        return div().child(working_row(p, "Reading the screen's size…"));
    };
    // The size field is always there when the display could not say its size,
    // and behind "Not right?" when it could.
    let field_open = !finding.is_found() || state.manual_size;
    let by_hand = state.reference().is_some();
    let found = finding.is_found();
    let lines: Vec<AnyElement> = finding
        .detail_lines()
        .into_iter()
        .map(|line| div().child(format!("\u{2022} {line}")).into_any_element())
        .collect();

    let mut col = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(finding_summary(p, state, &finding));

    if found && !field_open {
        col = col.child(div().child(link_button(
            "manual-size",
            p,
            "Not right? Enter the size yourself",
            act(wizard, |w, cx| {
                w.manual_size = true;
                cx.notify();
            }),
        )));
    }
    if found && !field_open && !by_hand {
        col = col.child(prefer_camera_toggle(p, wizard, cx));
    }
    if field_open {
        col = col.child(size_box.clone());
        if found {
            col = col.child(div().child(link_button(
                "tv-size",
                p,
                "Use the TV's own size instead",
                act(wizard, |w, cx| {
                    w.size_text.clear();
                    w.manual_size = false;
                    cx.notify();
                }),
            )));
        }
    }

    col.child(info_note(
        p,
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Before you start:"))
            .child("set the TV to Game or PC mode, or turn on Just Scan, so it doesn't crop the picture."),
    ))
    .child(plain_details("flat-details", p, "Details", lines, details_open, cx))
}

/// What the display said about its size, or which size is used instead.
fn finding_summary(p: Palette, state: &Wizard, finding: &Finding) -> Div {
    let found = finding.is_found();
    let typed = !state.size_text.trim().is_empty();
    let by_hand = state.reference().is_some();
    let prefer_camera = state.prefer_camera_size();
    let (glyph, ink) = if found {
        (Icon::Check, p.success_ink)
    } else {
        (Icon::Viewfinder, p.fg_muted)
    };
    let headline = if typed && found {
        "Using the size you entered instead of the TV's.".to_string()
    } else if by_hand {
        "Using your tape or card measurement instead of the TV's.".to_string()
    } else if prefer_camera && found {
        "The camera will measure the screen's size.".to_string()
    } else {
        finding.headline().to_string()
    };
    let mut text = div().flex().flex_col().gap_1().child(
        div()
            .text_lg()
            .font_weight(FontWeight::MEDIUM)
            .text_color(p.fg_strong)
            .child(headline),
    );
    if let Some(advice) = finding.advice() {
        text = text.child(
            div()
                .text_base()
                .text_color(p.fg_muted)
                .child(advice.to_string()),
        );
    }
    if let Some(aside) = finding.projector_aside() {
        text = text.child(
            div()
                .text_base()
                .text_color(p.fg_muted)
                .child(aside.to_string()),
        );
    }
    div()
        .flex()
        .items_start()
        .gap_3()
        .child(div().mt_0p5().child(icon(glyph, 24.0, ink)))
        .child(text)
}

/// The remembered choice to size flat screens with the camera instead of by
/// what the display reports.
fn prefer_camera_toggle(p: Palette, wizard: &Entity<Wizard>, cx: &App) -> Div {
    let prefer_camera = wizard.read(cx).prefer_camera_size();
    let wizard = wizard.clone();
    div()
        .flex()
        .w_auto()
        .items_start()
        .gap_3()
        .child(
            Checkbox::new("prefer-camera-size")
                .checked(prefer_camera)
                .on_click(move |checked, _, cx| {
                    let checked = *checked;
                    wizard.update(cx, |w, cx| w.set_prefer_camera_size(checked, cx));
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .text_base()
                .text_color(p.fg_strong)
                .child("Measure the size with the camera instead of using the TV's own")
                .child(
                    div()
                        .text_sm()
                        .text_color(p.fg_muted)
                        .child("Remembered for every screen. A size you type still wins."),
                ),
        )
}
