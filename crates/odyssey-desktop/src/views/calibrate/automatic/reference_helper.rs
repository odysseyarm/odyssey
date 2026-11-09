//! Setup's "measure the size by hand" helper: a tape measure on a line the
//! screen draws, or a credit card against a frame it draws.
//!
//! It sits under the size question so anyone who doubts the size sees it, and
//! asks for more when the display reported nothing usable. The controls live
//! here; the line or frame is shown by the pattern window on the monitor being
//! measured (see `PatternMode::Ruler` and `Card`).

use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::slider::{Slider, SliderEvent, SliderState};
use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, FontWeight, IntoElement, ParentElement as _,
    Render, Rgba, SharedString, Styled as _, Subscription, Window, div, px,
};

use super::kind::DisplayKind;
use super::reference::{self, Agreement, CARD_STEP_PX, Method, Reference, ReferenceError, Unit};
use super::ui::{act, act_boxed, amber_alert, link_button, segmented};
use super::wizard::{Helper, Wizard};
use crate::components::ui::{ButtonKind, button};
use crate::components::wizard::{Icon, icon, with_alpha};
use crate::theme::{Palette, RADIUS_LG, palette};

/// How far the slider may sit from the model's value before it is moved.
const SLIDER_EPSILON: f32 = 1e-4;

pub(super) struct ReferenceHelper {
    wizard: Entity<Wizard>,
    tape: Entity<InputState>,
    card: Entity<SliderState>,
    /// The unit the tape field's placeholder was last written for.
    placeholder_unit: Option<Unit>,
    _subscriptions: Vec<Subscription>,
}

impl ReferenceHelper {
    pub(super) fn new(wizard: Entity<Wizard>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tape = cx.new(|cx| InputState::new(window, cx));
        let card = cx.new(|_| SliderState::new().step(CARD_STEP_PX as f32));
        let subscriptions = vec![
            cx.observe(&wizard, |_, _, cx| cx.notify()),
            cx.subscribe_in(&tape, window, |this, tape, event: &InputEvent, _, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let text = tape.read(cx).value().to_string();
                this.wizard.update(cx, |wizard, cx| {
                    if let Some(unit) = wizard.helper.as_ref().map(|helper| helper.tape_unit) {
                        wizard.set_tape(text, unit, cx);
                    }
                });
            }),
            cx.subscribe(&card, |this, _, event: &SliderEvent, cx| {
                if let SliderEvent::Change(value) = event {
                    let px = f64::from(value.start());
                    this.wizard
                        .update(cx, |wizard, cx| wizard.set_card_px(px, cx));
                }
            }),
        ];
        Self {
            wizard,
            tape,
            card,
            placeholder_unit: None,
            _subscriptions: subscriptions,
        }
    }

    /// Mirrors the model into the two inputs. Programmatic sets emit no events,
    /// so this cannot echo back into the model.
    fn sync(&mut self, helper: &Helper, window: &mut Window, cx: &mut Context<Self>) {
        if self.tape.read(cx).value() != helper.tape_text.as_str() {
            let text = helper.tape_text.clone();
            self.tape
                .update(cx, |state, cx| state.set_value(text, window, cx));
        }
        if self.placeholder_unit.replace(helper.tape_unit) != Some(helper.tape_unit) {
            let placeholder = if helper.tape_unit == Unit::Inches {
                "47 5/8"
            } else {
                "121.0"
            };
            self.tape.update(cx, |state, cx| {
                state.set_placeholder(placeholder, window, cx);
            });
        }

        let (lo, hi) = (helper.card_bounds.0 as f32, helper.card_bounds.1 as f32);
        let value = helper.card_px as f32;
        let (state_lo, state_hi) = {
            let state = self.card.read(cx);
            (state.min_value(), state.max_value())
        };
        if (state_lo - lo).abs() > SLIDER_EPSILON || (state_hi - hi).abs() > SLIDER_EPSILON {
            self.card.update(cx, |state, cx| {
                *state = SliderState::new()
                    .max(hi)
                    .min(lo)
                    .step(CARD_STEP_PX as f32)
                    .default_value(value);
                cx.notify();
            });
        } else if (self.card.read(cx).value().start() - value).abs() > SLIDER_EPSILON {
            self.card
                .update(cx, |state, cx| state.set_value(value, window, cx));
        }
    }
}

impl Render for ReferenceHelper {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (helper, problem, reference, key, reading, finding, projected, panel) = {
            let wizard = self.wizard.read(cx);
            (
                wizard.helper.clone(),
                wizard.helper_problem.clone(),
                wizard.reference(),
                wizard.selected_monitor(),
                wizard.helper_reading(),
                wizard.finding.clone(),
                wizard.kind() == DisplayKind::Projector,
                wizard.panel_size_m(),
            )
        };

        let body = match helper {
            Some(helper) => {
                self.sync(&helper, window, cx);
                Some(self.open_helper(
                    p,
                    &helper,
                    reading,
                    key.as_ref().map(|k| (k.width, k.height)),
                    panel,
                ))
            }
            None => match (reference, &key) {
                (Some(reading), Some(key)) => Some(measured(
                    p,
                    &self.wizard,
                    reading,
                    (key.width, key.height),
                    panel,
                )),
                _ => finding.map(|finding| prompt(p, &self.wizard, projected, finding.is_found())),
            },
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(body)
            .children(problem.map(|problem| amber_alert(p, problem)))
    }
}

/// "123.4 cm (48.6 in) wide": a picture's width in both units a tech might think in.
fn width_words(size_m: [f64; 2]) -> String {
    format!(
        "{:.1} cm ({:.1} in) wide",
        size_m[0] * 100.0,
        size_m[0] / 0.0254
    )
}

/// How a hand measurement compares with the size the display reported; the
/// flag is set when the gap is large enough to warn about.
fn agreement_note(panel_size_m: Option<[f64; 2]>, size_m: [f64; 2]) -> Option<(String, bool)> {
    let gap = reference::size_gap(size_m, panel_size_m?);
    let percent = gap * 100.0;
    Some(match reference::agreement(gap) {
        Agreement::Close => (
            format!("Within {percent:.1}% of what the TV reports."),
            false,
        ),
        Agreement::Differs => (
            format!(
                "{percent:.1}% from what the TV reports. That's expected if the TV crops its picture, since you measured the whole picture, and displays are often a little off too."
            ),
            false,
        ),
        Agreement::FarOff => (
            format!(
                "{percent:.0}% from what the TV reports. A TV that crops a lot of its picture can do that. If it doesn't, check the unit and where you measured from."
            ),
            true,
        ),
    })
}

/// A rounded, bordered box for one state of the helper.
fn frame(border: Rgba, bg: Rgba) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(RADIUS_LG))
        .border_2()
        .border_color(border)
        .bg(bg)
        .p_4()
}

/// A numbered list of steps.
fn steps(p: Palette, items: Vec<&'static str>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .text_base()
        .text_color(p.fg)
        .children(items.into_iter().enumerate().map(|(i, text)| {
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .w(px(20.0))
                        .flex_shrink_0()
                        .child(format!("{}.", i + 1)),
                )
                .child(div().flex_1().child(text))
        }))
}

/// The invitation: always there for a flat screen or a projector, louder for a
/// projector or a display that reported no usable size.
fn prompt(p: Palette, wizard: &Entity<Wizard>, projected: bool, found: bool) -> AnyElement {
    let silent = projected || !found;
    let (title, detail) = if projected {
        (
            "Know the picture's size exactly?",
            "The camera measures a projected picture. A tape measure or a credit card takes about a minute and gives it an exact size.",
        )
    } else if silent {
        (
            "Measure the size by hand for the best result",
            "The TV didn't report a usable size, so the camera would measure it. A tape measure or a credit card takes about a minute and is usually more exact.",
        )
    } else {
        (
            "Want to be sure of the size?",
            "The TV reports its size, but displays are sometimes a little off. A tape measure or a credit card takes about a minute and checks it.",
        )
    };
    let (border, bg) = if silent {
        (with_alpha(p.warning, 0.4), p.warning_soft)
    } else {
        (p.line, p.surface_alt)
    };
    frame(border, bg)
        .gap_3()
        .child(
            div()
                .flex()
                .items_start()
                .gap_3()
                .child(
                    div()
                        .flex_shrink_0()
                        .mt_0p5()
                        .child(icon(Icon::Bulb, 24.0, p.info_ink)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.fg_strong)
                                .child(title),
                        )
                        .child(div().text_base().text_color(p.fg_muted).child(detail)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .child(
                    button(
                        "helper-tape",
                        "Use a tape measure",
                        ButtonKind::Secondary,
                        p,
                    )
                    .on_click(act(wizard, |wizard, cx| {
                        wizard.open_helper(Method::Tape, cx);
                    })),
                )
                .child(
                    button("helper-card", "Use a credit card", ButtonKind::Secondary, p).on_click(
                        act(wizard, |wizard, cx| wizard.open_helper(Method::Card, cx)),
                    ),
                ),
        )
        .child(
            div()
                .text_sm()
                .text_color(p.fg_muted)
                .child("The tape is the most exact; the card is quicker."),
        )
        .into_any_element()
}

/// The result in use, with a way to redo or drop it.
fn measured(
    p: Palette,
    wizard: &Entity<Wizard>,
    reading: Reference,
    (width, height): (u32, u32),
    panel: Option<[f64; 2]>,
) -> AnyElement {
    let size = reading.size_m(width, height);
    let how = match reading.method {
        Method::Tape => "tape measure",
        Method::Card => "credit card",
    };
    let note = agreement_note(panel, size).map(|(text, warn)| {
        div()
            .text_base()
            .text_color(if warn { p.warning_ink } else { p.fg_muted })
            .child(text)
    });
    let method = reading.method;
    frame(with_alpha(p.success, 0.4), p.success_soft)
        .gap_2()
        .child(
            div()
                .flex()
                .items_start()
                .gap_3()
                .child(div().mt_0p5().child(icon(Icon::Check, 24.0, p.success_ink)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.fg_strong)
                                .child(format!(
                                    "Using your {how} measurement: {}.",
                                    width_words(size)
                                )),
                        )
                        .children(note),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_4()
                .child(link_button(
                    "helper-again",
                    p,
                    "Measure again",
                    act(wizard, move |wizard, cx| wizard.open_helper(method, cx)),
                ))
                .child(link_button(
                    "helper-drop",
                    p,
                    "Don't use it",
                    act(wizard, Wizard::clear_reference),
                )),
        )
        .into_any_element()
}

impl ReferenceHelper {
    fn open_helper(
        &self,
        p: Palette,
        helper: &Helper,
        reading: Option<Result<Reference, ReferenceError>>,
        monitor: Option<(u32, u32)>,
        panel: Option<[f64; 2]>,
    ) -> AnyElement {
        let wizard = &self.wizard;
        let method = helper.method;
        let typed = !helper.tape_text.trim().is_empty();
        let usable = matches!(reading, Some(Ok(_))) && (method == Method::Card || typed);
        let preview = match (&reading, monitor) {
            (Some(Ok(r)), Some((width, height))) if method == Method::Card || typed => {
                let size = r.size_m(width, height);
                Some((width_words(size), agreement_note(panel, size)))
            }
            _ => None,
        };
        let error = match (&reading, method, typed) {
            (Some(Err(e)), Method::Tape, true) => Some(e.to_string()),
            _ => None,
        };

        let method_choice = segmented(
            p,
            "helper-method",
            [
                (Method::Tape, "Tape measure"),
                (Method::Card, "Credit card"),
            ]
            .into_iter()
            .map(|(option, label)| {
                (
                    SharedString::from(label),
                    option == method,
                    act_boxed(wizard, move |wizard, cx| {
                        wizard.set_helper_method(option, cx);
                    }),
                )
            })
            .collect(),
            16.0,
        );
        let how = match method {
            Method::Tape => self.tape_steps(p, helper),
            Method::Card => self.card_steps(p, helper),
        };

        let preview = preview.map(|(width, note)| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_base()
                        .text_color(p.fg)
                        .child(format!("That makes the picture {width}.")),
                )
                .children(note.map(|(text, warn)| {
                    let line = div().child(text);
                    if warn {
                        line.text_base().text_color(p.warning_ink)
                    } else {
                        line.text_sm().text_color(p.fg_muted)
                    }
                }))
        });

        frame(with_alpha(p.accent_ink, 0.4), p.accent_soft)
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.fg_strong)
                            .child("Measure the size by hand"),
                    )
                    .child(method_choice),
            )
            .child(how)
            .children(preview)
            .children(error.map(|error| div().text_base().text_color(p.danger_ink).child(error)))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(
                        button("helper-use", "Use this size", ButtonKind::Primary, p)
                            .disabled(!usable)
                            .on_click(act(wizard, Wizard::apply_helper)),
                    )
                    .child(
                        button("helper-cancel", "Cancel", ButtonKind::Secondary, p)
                            .on_click(act(wizard, Wizard::cancel_helper)),
                    ),
            )
            .into_any_element()
    }

    fn tape_steps(&self, p: Palette, helper: &Helper) -> Div {
        let wizard = &self.wizard;
        let unit = helper.tape_unit;
        let list = if helper.covers {
            vec![
                "Press \u{201c}Show the line\u{201d}. The screen shows a line with a tick at each end, over this window.",
                "Lay the tape along the line and read the length from the middle of one tick to the middle of the other. Then press Escape to come back.",
                "Type what you read.",
            ]
        } else {
            vec![
                "Look at the screen: it shows a line with a tick at each end.",
                "Lay the tape along the line and read the length from the middle of one tick to the middle of the other.",
                "Type what you read.",
            ]
        };
        let units = segmented(
            p,
            "helper-unit",
            [
                (Unit::Inches, "in"),
                (Unit::Centimeters, "cm"),
                (Unit::Millimeters, "mm"),
            ]
            .into_iter()
            .map(|(option, label)| {
                let text = helper.tape_text.clone();
                (
                    SharedString::from(label),
                    option == unit,
                    act_boxed(wizard, move |wizard, cx| {
                        wizard.set_tape(text.clone(), option, cx);
                    }),
                )
            })
            .collect(),
            12.0,
        );
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(steps(p, list))
            .children(helper.covers.then(|| {
                div().child(
                    button(
                        "helper-show-line",
                        "Show the line",
                        ButtonKind::Secondary,
                        p,
                    )
                    .on_click(act(wizard, Wizard::show_helper_picture)),
                )
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(div().w(px(160.0)).child(Input::new(&self.tape)))
                    .child(units),
            )
    }

    fn card_steps(&self, p: Palette, helper: &Helper) -> Div {
        let wizard = &self.wizard;
        let card_px = helper.card_px;
        let list = if helper.covers {
            vec![
                "Take any bank card, driver's licence or ID card.",
                "Press \u{201c}Show the frame\u{201d}. The screen shows a white frame, over this window.",
                "Hold the card flat against the glass inside the frame and look straight at it. Press the arrow keys until the frame hugs the card, with no dark gap and none of the frame covered (Page Up and Page Down move it faster), then press Enter to come back.",
            ]
        } else {
            vec![
                "Take any bank card, driver's licence or ID card.",
                "Hold it flat against the glass inside the white frame on the screen, and look straight at it.",
                "Nudge the frame until it hugs the card: no dark gap, and none of the frame covered.",
            ]
        };
        let width = div()
            .text_sm()
            .text_color(p.fg_muted)
            .child(format!("{card_px:.2} pixels wide"));
        let controls = if helper.covers {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_3()
                .child(
                    button(
                        "helper-show-frame",
                        "Show the frame",
                        ButtonKind::Secondary,
                        p,
                    )
                    .on_click(act(wizard, Wizard::show_helper_picture)),
                )
                .child(width)
        } else {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_3()
                .child(
                    button("helper-smaller", "\u{2212}", ButtonKind::Secondary, p).on_click(act(
                        wizard,
                        move |wizard, cx| {
                            wizard.set_card_px(card_px - CARD_STEP_PX, cx);
                        },
                    )),
                )
                .child(
                    div()
                        .w(px(256.0))
                        .max_w_full()
                        .child(Slider::new(&self.card)),
                )
                .child(
                    button("helper-larger", "+", ButtonKind::Secondary, p).on_click(act(
                        wizard,
                        move |wizard, cx| {
                            wizard.set_card_px(card_px + CARD_STEP_PX, cx);
                        },
                    )),
                )
                .child(width)
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(steps(p, list))
            .child(controls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_is_given_in_both_units() {
        assert_eq!(width_words([1.0, 0.5]), "100.0 cm (39.4 in) wide");
    }
}
