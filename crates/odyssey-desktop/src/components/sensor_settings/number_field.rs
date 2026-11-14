//! Form widgets of the settings panels. The `SettingsForm` strings stay the
//! source of truth. Each widget mirrors one of them, writing edits back and
//! being refilled by `push_*` after the form changes underneath it.

use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::select::{Select, SelectEvent, SelectState};
use gpui::component::{IndexPath, Sizable as _};
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, SharedString,
    Styled as _, Subscription, Window, div, px,
};

use super::settings_form::SettingsForm;
use super::settings_panel::SensorSettings;
use crate::theme::{Palette, RADIUS_SM};

/// Width of the label column, so the fields of one panel line up.
const LABEL_WIDTH: f32 = 176.0;

pub(super) type OptionSelect = Entity<SelectState<Vec<SharedString>>>;

/// A text input that writes every user edit into the form string `slot` picks.
pub(super) fn bind_text(
    window: &mut Window,
    cx: &mut Context<SensorSettings>,
    subscriptions: &mut Vec<Subscription>,
    slot: impl Fn(&mut SettingsForm) -> &mut String + 'static,
) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx));
    subscriptions.push(cx.subscribe_in(
        &input,
        window,
        move |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                *slot(&mut this.form) = input.read(cx).value().to_string();
                cx.notify();
            }
        },
    ));
    input
}

/// A dropdown over `options` that writes the index of the chosen option, as
/// text, into the form string `slot` picks.
pub(super) fn bind_select(
    window: &mut Window,
    cx: &mut Context<SensorSettings>,
    subscriptions: &mut Vec<Subscription>,
    options: Vec<SharedString>,
    slot: impl Fn(&mut SettingsForm) -> &mut String + 'static,
) -> OptionSelect {
    let choices = options.clone();
    let select = cx.new(|cx| SelectState::new(options, None, window, cx));
    subscriptions.push(cx.subscribe_in(
        &select,
        window,
        move |this, _, event: &SelectEvent<Vec<SharedString>>, _, cx| {
            let SelectEvent::Confirm(Some(chosen)) = event else {
                return;
            };
            if let Some(index) = choices.iter().position(|option| option == chosen) {
                *slot(&mut this.form) = index.to_string();
                cx.notify();
            }
        },
    ));
    select
}

/// Shows `value` in the input. Programmatic sets do not emit `Change`, so the
/// form is not written back.
pub(super) fn push_text(
    input: &Entity<InputState>,
    value: &str,
    window: &mut Window,
    cx: &mut App,
) {
    if input.read(cx).value() != value {
        input.update(cx, |state, cx| {
            state.set_value(value.to_owned(), window, cx);
        });
    }
}

/// Selects the option whose index is the text `value`, or none when it is not
/// a valid index.
pub(super) fn push_select(select: &OptionSelect, value: &str, window: &mut Window, cx: &mut App) {
    let index = value
        .trim()
        .parse::<usize>()
        .ok()
        .map(|row| IndexPath::default().row(row));
    select.update(cx, |state, cx| state.set_selected_index(index, window, cx));
}

fn row(label: &str, field: impl IntoElement, palette: &Palette) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .w(px(LABEL_WIDTH))
                .flex_shrink_0()
                .pt_1p5()
                .text_sm()
                .whitespace_nowrap()
                .text_color(palette.fg_muted)
                .child(label.to_owned()),
        )
        .child(div().flex_1().min_w_0().child(field))
}

/// Marks a widget with a red outline while it has an issue.
fn outlined(invalid: bool, palette: &Palette, widget: impl IntoElement) -> impl IntoElement {
    div()
        .flex_1()
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(if invalid {
            palette.danger
        } else {
            gpui::transparent_black().into()
        })
        .child(widget)
}

fn issue_line(issue: Option<String>, palette: &Palette) -> Option<impl IntoElement> {
    issue.map(|message| {
        div()
            .text_xs()
            .text_color(palette.danger_ink)
            .child(message)
    })
}

/// A labelled numeric field with an optional unit hint and validation message.
///
/// `hint` carries the converted engineering value (ms, fps) so the raw register
/// numbers stop being the only thing on screen.
pub(super) fn number_row(
    label: &str,
    input: &Entity<InputState>,
    hint: Option<String>,
    issue: Option<String>,
    palette: &Palette,
) -> impl IntoElement {
    let field = div()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(outlined(
                    issue.is_some(),
                    palette,
                    Input::new(input).small(),
                ))
                .children(hint.map(|hint| {
                    div()
                        .text_xs()
                        .whitespace_nowrap()
                        .text_color(palette.fg_muted)
                        .child(hint)
                })),
        )
        .children(issue_line(issue, palette));
    row(label, field, palette)
}

/// A labelled dropdown with an optional validation message.
pub(super) fn select_row(
    label: &str,
    select: &OptionSelect,
    issue: Option<String>,
    palette: &Palette,
) -> impl IntoElement {
    let field = div()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(outlined(
            issue.is_some(),
            palette,
            Select::new(select).small(),
        ))
        .children(issue_line(issue, palette));
    row(label, field, palette)
}
