//! The shot delay stepper and its Set, Reset and Save buttons.

use gpui::component::input::Input;
use gpui::{
    Context, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use super::device_controls::{DeviceControls, Tone, pill};
use super::device_state::MAX_SHOT_DELAY_MS;
use crate::runtime;
use crate::theme::{Palette, RADIUS};

/// Width of the stepper: the input and both step buttons.
const STEPPER_WIDTH: f32 = 96.0;
const STEPPER_HEIGHT: f32 = 40.0;

/// The shot delay a typed value stands for: out-of-range numbers stick to the
/// nearest end of the range.
fn clamp_shot_delay(typed: i32) -> u16 {
    u16::try_from(typed.clamp(0, i32::from(MAX_SHOT_DELAY_MS))).unwrap_or(MAX_SHOT_DELAY_MS)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

fn step_button(
    id: impl Into<ElementId>,
    glyph: &'static str,
    side: Side,
    disabled: bool,
    p: Palette,
) -> gpui::Stateful<gpui::Div> {
    let button = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .h(px(STEPPER_HEIGHT))
        .px_3()
        .border_1()
        .border_color(Hsla::from(p.fg_faint).opacity(0.7))
        .bg(p.surface_alt)
        .text_color(p.fg_strong)
        .child(glyph);
    let button = match side {
        Side::Left => button.rounded_tl(px(RADIUS)).rounded_bl(px(RADIUS)),
        Side::Right => button.rounded_tr(px(RADIUS)).rounded_br(px(RADIUS)),
    };
    if disabled {
        return button.opacity(0.5);
    }
    button
        .cursor_pointer()
        .hover(move |style| style.bg(p.surface_hover))
}

impl DeviceControls {
    /// Pushes a delay that did not come from the input (a step, a reset, the
    /// stored value) into the input's text.
    pub(super) fn show_shot_delay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let revision = self.state.shot_delay_revision();
        if self.shown_revision == revision {
            return;
        }
        self.shown_revision = revision;
        let text = self.state.shot_delay_ms().to_string();
        self.shot_delay_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    /// The input's text changed. Anything that is not a number is left alone;
    /// numbers outside the range are replaced by the nearest end.
    pub(super) fn shot_delay_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.shot_delay_input.read(cx).value();
        let Ok(number) = typed.parse::<i32>() else {
            return;
        };
        let delay_ms = clamp_shot_delay(number);
        self.state.set_shot_delay_ms(delay_ms);
        self.shown_revision = self.state.shot_delay_revision();
        let shown = delay_ms.to_string();
        if *typed != *shown {
            self.shot_delay_input
                .update(cx, |input, cx| input.set_value(shown, window, cx));
        }
        cx.notify();
    }

    fn step_shot_delay(&mut self, up: bool, cx: &mut Context<Self>) {
        let delay_ms = self.state.shot_delay_ms();
        if up && delay_ms < MAX_SHOT_DELAY_MS {
            self.state.set_shot_delay_ms(delay_ms + 1);
        } else if !up && delay_ms > 0 {
            self.state.set_shot_delay_ms(delay_ms - 1);
        }
        cx.notify();
    }

    /// Set-and-save is one device operation: another Set must not replace its
    /// value between the two RPCs. The controls are reserved before the
    /// command starts and released when `apply` has run.
    fn run_command<T: Send + 'static>(
        &mut self,
        command: impl Future<Output = T> + Send + 'static,
        apply: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.pending = true;
        cx.notify();
        let done = runtime::spawn(command);
        cx.spawn(async move |this, cx| {
            let result = done.await;
            this.update(cx, |controls, cx| {
                controls.pending = false;
                match result {
                    Ok(value) => apply(controls, value, cx),
                    Err(e) => tracing::error!("Shot delay command stopped: {e}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_shot_delay(&mut self, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let Some(client) = self.client("set shot delay", cx) else {
            return;
        };
        let device = self.device.clone();
        let delay_ms = self.state.shot_delay_ms();
        self.run_command(
            async move {
                match client.device(device.clone()).set_shot_delay(delay_ms).await {
                    Ok(()) => {
                        tracing::info!("Set shot delay {}ms for {:x?}", delay_ms, device.uuid)
                    }
                    Err(e) => tracing::error!("Failed to set shot delay {:x?}: {}", device.uuid, e),
                }
            },
            |_, (), _| {},
            cx,
        );
    }

    fn reset_shot_delay(&mut self, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let Some(client) = self.client("reset shot delay", cx) else {
            return;
        };
        let device = self.device.clone();
        let revision = self.state.shot_delay_revision();
        self.run_command(
            async move {
                match client.device(device.clone()).reset_shot_delay().await {
                    Ok(delay_ms) => {
                        tracing::info!("Reset shot delay for {:x?}", device.uuid);
                        Some(delay_ms)
                    }
                    Err(e) => {
                        tracing::error!("Failed to reset shot delay {:x?}: {}", device.uuid, e);
                        None
                    }
                }
            },
            move |controls, delay_ms, _| {
                if let Some(delay_ms) = delay_ms
                    && controls.state.shot_delay_revision() == revision
                {
                    controls.state.set_shot_delay_ms(delay_ms);
                }
            },
            cx,
        );
    }

    fn set_and_save_shot_delay(&mut self, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let Some(client) = self.client("set shot delay", cx) else {
            return;
        };
        let device = self.device.clone();
        let delay_ms = self.state.shot_delay_ms();
        self.run_command(
            async move {
                let client = client.device(device.clone());
                if let Err(e) = client.set_shot_delay(delay_ms).await {
                    tracing::error!("Failed to set shot delay {:x?}: {}", device.uuid, e);
                    return;
                }
                tracing::info!("Set shot delay {}ms for {:x?}", delay_ms, device.uuid);
                match client.save_shot_delay().await {
                    Ok(()) => tracing::info!("Saved shot delay for {:x?}", device.uuid),
                    Err(e) => {
                        tracing::error!("Failed to save shot delay {:x?}: {}", device.uuid, e)
                    }
                }
            },
            |_, (), _| {},
            cx,
        );
    }

    pub(super) fn shot_delay_row(
        &mut self,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = cx.entity_id();
        let pending = self.pending;
        let decrement = step_button(("zero-delay-decrement", id), "−", Side::Left, pending, p)
            .on_click(cx.listener(|controls, _, _, cx| {
                if !controls.pending {
                    controls.step_shot_delay(false, cx);
                }
            }));
        let increment = step_button(("zero-delay-increment", id), "+", Side::Right, pending, p)
            .on_click(cx.listener(|controls, _, _, cx| {
                if !controls.pending {
                    controls.step_shot_delay(true, cx);
                }
            }));
        let set = pill(("zero-delay-set", id), "Set", Tone::Plain, pending, p)
            .on_click(cx.listener(|controls, _, _, cx| controls.set_shot_delay(cx)));
        let reset = pill(("zero-delay-reset", id), "Reset", Tone::Plain, pending, p)
            .on_click(cx.listener(|controls, _, _, cx| controls.reset_shot_delay(cx)));
        let save = pill(("zero-delay-save", id), "Save", Tone::Plain, pending, p)
            .on_click(cx.listener(|controls, _, _, cx| controls.set_and_save_shot_delay(cx)));

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(p.fg_strong)
                    .child("Shot Delay (ms):"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(decrement)
                    .child(
                        div().w(px(STEPPER_WIDTH)).child(
                            Input::new(&self.shot_delay_input)
                                .disabled(pending)
                                .h(px(STEPPER_HEIGHT)),
                        ),
                    )
                    .child(increment),
            )
            .child(set)
            .child(reset)
            .child(save)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_delays_stick_to_the_range() {
        assert_eq!(clamp_shot_delay(-5), 0);
        assert_eq!(clamp_shot_delay(0), 0);
        assert_eq!(clamp_shot_delay(250), 250);
        assert_eq!(clamp_shot_delay(1023), 1023);
        assert_eq!(clamp_shot_delay(1024), 1023);
        assert_eq!(clamp_shot_delay(i32::MAX), 1023);
    }
}
