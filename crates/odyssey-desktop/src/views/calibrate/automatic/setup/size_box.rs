//! The optional size box, in the unit of the field being asked for.

use gpui::component::input::{Input, InputEvent, InputState};
use gpui::{
    AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div, px,
};

use crate::theme::palette;
use crate::views::calibrate::automatic::kind::{DisplayKind, Finding};
use crate::views::calibrate::automatic::settings::{
    DIAGONAL_RANGE_IN, SizeError, SizeField, WIDTH_RANGE_CM,
};
use crate::views::calibrate::automatic::wizard::Wizard;

pub(super) struct SizeBox {
    wizard: Entity<Wizard>,
    input: Entity<InputState>,
    /// A half-typed number such as the "4" of 40 is out of range for a moment.
    /// That is reported once the tech leaves the box, not while they type.
    left_box: bool,
    _subscriptions: Vec<Subscription>,
}

impl SizeBox {
    pub(super) fn new(wizard: Entity<Wizard>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Measure it for me"));
        let subscriptions = vec![
            cx.observe(&wizard, |_, _, cx| cx.notify()),
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    this.left_box = false;
                    let text = input.read(cx).value().to_string();
                    this.wizard.update(cx, |wizard, cx| {
                        wizard.size_text = text;
                        cx.notify();
                    });
                }
                InputEvent::Blur => {
                    this.left_box = true;
                    cx.notify();
                }
                _ => {}
            }),
        ];
        Self {
            wizard,
            input,
            left_box: false,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for SizeBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &palette(cx);
        let wizard = self.wizard.read(cx);
        let field = wizard.field();
        let label = match (wizard.kind(), field) {
            (DisplayKind::Flat, _) if wizard.finding.as_ref().is_some_and(Finding::is_found) => {
                "Screen size (diagonal)"
            }
            (DisplayKind::Flat, _) => "Size printed on the box (optional)",
            (DisplayKind::Projector, SizeField::Diagonal) => "Picture size (diagonal)",
            (DisplayKind::Projector, SizeField::Width) => "Picture width",
        };
        let error = wizard
            .entered()
            .err()
            .filter(|e| self.left_box || matches!(e, SizeError::NotANumber))
            .map(SizeError::message);
        let (unit, range) = match field {
            SizeField::Diagonal => (
                "inches",
                format!("{}–{}", DIAGONAL_RANGE_IN.start(), DIAGONAL_RANGE_IN.end()),
            ),
            SizeField::Width => (
                "cm",
                format!("{}–{}", WIDTH_RANGE_CM.start(), WIDTH_RANGE_CM.end()),
            ),
        };
        let text = wizard.size_text.clone();
        if self.input.read(cx).value() != text.as_str() {
            self.input
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }

        let mut col = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.fg)
                    .child(label),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(192.0)).child(Input::new(&self.input)))
                    .child(
                        div()
                            .text_lg()
                            .text_color(p.fg_muted)
                            .child(format!("{unit} ({range})")),
                    ),
            );
        if let Some(error) = error {
            col = col.child(div().text_sm().text_color(p.danger_ink).child(error));
        }
        col
    }
}
