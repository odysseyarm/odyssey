//! A step failed. Says what happened in plain words and offers the one sensible
//! retry. The raw error stays under Details for support.

use gpui::{AnyElement, App, Div, Entity, IntoElement as _, ParentElement as _, Styled as _, div};

use super::attempt_button::save_attempt;
use super::hints::{failure_message, retry_label};
use super::ui::{act, details, step_page};
use super::wizard::Wizard;
use crate::components::ui::{ButtonKind, button};
use crate::components::wizard::status_banner;
use crate::theme::Palette;

pub(super) fn problem_step(
    p: &Palette,
    wizard: &Entity<Wizard>,
    state: &Wizard,
    details_open: &Entity<bool>,
    cx: &App,
) -> Div {
    let Some(failure) = state.failure.as_ref() else {
        return div();
    };
    let message = failure_message(failure.kind, failure.size_origin);
    let raw: AnyElement = div()
        .text_sm()
        .child(failure.details.clone())
        .into_any_element();
    step_page(
        p,
        "That didn't work",
        None,
        vec![
            status_banner(p, message.tone, message.icon, message.title, message.detail)
                .into_any_element(),
            details("problem-details", p, "Details", vec![raw], details_open, cx)
                .into_any_element(),
            save_attempt(p, wizard, state).into_any_element(),
        ],
        vec![
            button("back-to-setup", "Back to setup", ButtonKind::Secondary, p)
                .on_click(act(wizard, Wizard::back_to_setup))
                .into_any_element(),
            button("retry", retry_label(failure.kind), ButtonKind::Primary, p)
                .on_click(act(wizard, Wizard::retry))
                .into_any_element(),
        ],
    )
}
