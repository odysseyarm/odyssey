//! Shown while a measurement runs, first the screen and then the markers.
//! Progress in words and a bar, and a Cancel button that stops everything.

use gpui::{Div, Entity, IntoElement as _, ParentElement as _, Styled as _, div, px};

use super::hints::{Progress, progress};
use super::ui::{act, step_page};
use super::wizard::Wizard;
use crate::components::ui::{ButtonKind, button, card, progress_bar};
use crate::components::wizard::spinner;
use crate::theme::Palette;

/// `what` is what is being measured, for the progress sentence: "the screen".
pub(super) fn measuring_step(
    p: Palette,
    wizard: &Entity<Wizard>,
    state: &Wizard,
    title: &'static str,
    what: &str,
    hint: &'static str,
) -> Div {
    // Until the camera thread reports its first step there is nothing to count.
    let shown = match state.progress {
        Some(step) => progress(step, what),
        None => Progress {
            text: "Getting ready…".into(),
            fraction: 0.0,
        },
    };
    let percent = (shown.fraction * 100.0).round() as usize;

    step_page(
        p,
        title,
        Some(hint.into()),
        vec![
            card(p)
                .items_center()
                .gap_6()
                .px_6()
                .py(px(56.0))
                .child(spinner(56.0, p.accent_ink))
                .child(
                    div()
                        .text_2xl()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(p.fg_strong)
                        .child(shown.text),
                )
                .child(div().w_full().max_w(px(448.0)).child(progress_bar(p, percent)))
                .into_any_element(),
        ],
        vec![
            button("cancel", "Cancel", ButtonKind::Secondary, p)
                .on_click(act(wizard, |w, cx| w.back_to_setup(cx)))
                .into_any_element(),
            div().into_any_element(),
        ],
    )
}
