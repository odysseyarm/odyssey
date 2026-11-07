//! "What are you calibrating?": a flat screen or a projector.

use gpui::{
    App, Div, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::components::wizard::{Icon, icon};
use crate::theme::{Palette, RADIUS_LG};
use crate::views::calibrate::automatic::illustrations::{projector_illustration, tv_illustration};
use crate::views::calibrate::automatic::kind::DisplayKind;
use crate::views::calibrate::automatic::wizard::Wizard;

const KINDS: [DisplayKind; 2] = [DisplayKind::Flat, DisplayKind::Projector];

fn kind_title(kind: DisplayKind) -> &'static str {
    match kind {
        DisplayKind::Flat => "TV or monitor",
        DisplayKind::Projector => "Projector",
    }
}

fn kind_blurb(kind: DisplayKind) -> &'static str {
    match kind {
        DisplayKind::Flat => "A flat screen with a fixed size",
        DisplayKind::Projector => "A picture thrown onto a wall or screen",
    }
}

fn kind_id(kind: DisplayKind) -> &'static str {
    match kind {
        DisplayKind::Flat => "kind-flat",
        DisplayKind::Projector => "kind-projector",
    }
}

/// Two big illustrated cards behind one radio group. Arrow keys move the
/// choice, and only the chosen card is a tab stop, as for any radio group.
pub(super) fn kind_chooser(
    p: Palette,
    wizard: &Entity<Wizard>,
    focus: &[FocusHandle; 2],
    cx: &App,
) -> Div {
    let chosen = wizard.read(cx).kind();
    let mut row = div().grid().grid_cols(2).gap_3();
    for (i, kind) in KINDS.into_iter().enumerate() {
        let selected = kind == chosen;
        let (border, bg) = if selected {
            (p.accent_ink, p.accent_soft)
        } else {
            (p.line, p.surface)
        };
        let key_wizard = wizard.clone();
        let click_wizard = wizard.clone();
        let handles = focus.clone();
        let mut card = div()
            .id(kind_id(kind))
            .track_focus(&focus[i])
            .tab_stop(selected)
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .px_4()
            .pb_4()
            .pt_5()
            .text_center()
            .rounded(px(RADIUS_LG))
            .border_2()
            .border_color(border)
            .bg(bg)
            .cursor_pointer()
            .on_key_down(move |event, window, cx| {
                let step = match event.keystroke.key.as_str() {
                    "right" | "down" => 1,
                    "left" | "up" => KINDS.len() - 1,
                    _ => return,
                };
                cx.stop_propagation();
                let at = KINDS
                    .iter()
                    .position(|&k| k == key_wizard.read(cx).kind())
                    .unwrap_or(0);
                let next = (at + step) % KINDS.len();
                key_wizard.update(cx, |w, cx| w.set_kind(KINDS[next], cx));
                handles[next].focus(window, cx);
            })
            .on_click(move |_, _, cx| click_wizard.update(cx, |w, cx| w.set_kind(kind, cx)));
        if !selected {
            card = card.hover(move |s| s.border_color(p.accent_ink));
        }
        if selected {
            card = card.child(div().absolute().right_3().top_3().child(icon(
                Icon::Check,
                28.0,
                p.accent_ink,
            )));
        }
        let art = match kind {
            DisplayKind::Flat => tv_illustration(p, 112.0).into_any_element(),
            DisplayKind::Projector => projector_illustration(p, 112.0).into_any_element(),
        };
        row = row.child(
            card.child(art)
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.fg_strong)
                        .child(kind_title(kind)),
                )
                .child(
                    div()
                        .text_base()
                        .text_color(p.fg_muted)
                        .child(kind_blurb(kind)),
                ),
        );
    }
    row
}
