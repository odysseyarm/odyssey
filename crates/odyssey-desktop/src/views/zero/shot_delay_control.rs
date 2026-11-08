use std::time::Duration;

use gpui::component::input::Input;
use gpui::{Context, IntoElement, ParentElement as _, Styled as _, Window, div, px};

use super::device_controls::DeviceControls;
use super::device_state::MAX_SHOT_DELAY_MS;
use crate::components::ui::{ButtonKind, button};
use crate::runtime;
use crate::theme::Palette;

fn clamp_shot_delay(typed: i32) -> u16 {
    u16::try_from(typed.clamp(0, i32::from(MAX_SHOT_DELAY_MS))).unwrap_or(MAX_SHOT_DELAY_MS)
}

impl DeviceControls {
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

    fn run_command<T: Send + 'static>(
        &mut self,
        command: impl Future<Output = Result<T, String>> + Send + 'static,
        apply: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.pending = true;
        self.delay_error = None;
        self.delay_notice = Some("Updating shot delay…");
        cx.notify();
        let disconnected = self.disconnected.clone();
        let done = runtime::spawn(async move {
            tokio::select! {
                () = disconnected.cancelled() => Err("Tracker disconnected. Reload shot delay after reconnecting.".to_owned()),
                result = tokio::time::timeout(Duration::from_secs(10), command) => result.unwrap_or_else(|_| Err("Shot delay command timed out. Reload before capturing.".to_owned())),
            }
        });
        let controls = cx.entity();
        cx.spawn(async move |_, cx| {
            let result = done.await.unwrap_or_else(|error| Err(error.to_string()));
            controls.update(cx, |controls, cx| {
                controls.pending = false;
                controls.delay_notice = None;
                match result {
                    Ok(value) => apply(controls, value, cx),
                    Err(error) => {
                        controls.delay_loaded = false;
                        controls.delay_error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn load_shot_delay(&mut self, cx: &mut Context<Self>) {
        if self.pending || self.state.pending.is_some() {
            return;
        }
        let Some(client) = self.client(cx) else {
            return;
        };
        let device = self.device.clone();
        let revision = self.state.shot_delay_revision();
        self.run_command(
            async move {
                client
                    .device(device)
                    .get_shot_delay()
                    .await
                    .map_err(|error| format!("Could not load shot delay: {error}"))
            },
            move |controls, delay_ms, _| {
                controls.delay_loaded = true;
                if controls.state.shot_delay_revision() == revision {
                    controls.state.set_shot_delay_ms(delay_ms);
                }
            },
            cx,
        );
    }

    fn set_shot_delay(&mut self, cx: &mut Context<Self>) {
        if self.pending || self.state.pending.is_some() || self.state.armed {
            return;
        }
        let Some(client) = self.client(cx) else {
            return;
        };
        let device = self.device.clone();
        let delay_ms = self.state.shot_delay_ms();
        self.run_command(
            async move {
                client
                    .device(device)
                    .set_shot_delay(delay_ms)
                    .await
                    .map_err(|error| format!("Shot delay not set: {error}. Retry Set."))
            },
            |controls, (), _| {
                controls.delay_loaded = true;
                controls.delay_notice =
                    Some("Shot delay set for this session. Save delay to persist it.");
            },
            cx,
        );
    }

    fn reset_shot_delay(&mut self, cx: &mut Context<Self>) {
        if self.pending || self.state.pending.is_some() || self.state.armed {
            return;
        }
        let Some(client) = self.client(cx) else {
            return;
        };
        let device = self.device.clone();
        let revision = self.state.shot_delay_revision();
        self.run_command(
            async move {
                client
                    .device(device)
                    .reset_shot_delay()
                    .await
                    .map_err(|error| format!("Shot delay not reset: {error}. Retry Reset."))
            },
            move |controls, delay_ms, _| {
                controls.delay_loaded = true;
                controls.delay_notice = Some("Default shot delay restored; not saved.");
                if controls.state.shot_delay_revision() == revision {
                    controls.state.set_shot_delay_ms(delay_ms);
                }
            },
            cx,
        );
    }

    fn set_and_save_shot_delay(&mut self, cx: &mut Context<Self>) {
        if self.pending || self.state.pending.is_some() || self.state.armed {
            return;
        }
        let Some(client) = self.client(cx) else {
            return;
        };
        let device = self.device.clone();
        let delay_ms = self.state.shot_delay_ms();
        self.run_command(
            async move {
                let client = client.device(device);
                client.set_shot_delay(delay_ms).await.map_err(|error| {
                    format!("Shot delay not set or saved: {error}. Retry Save delay.")
                })?;
                client.save_shot_delay().await.map_err(|error| {
                    format!("Shot delay set but not saved: {error}. Retry Save delay.")
                })
            },
            |controls, (), _| {
                controls.delay_loaded = true;
                controls.delay_notice = Some("Shot delay saved.");
            },
            cx,
        );
    }

    pub(super) fn shot_delay_row(
        &mut self,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let busy =
            self.pending || self.state.pending.is_some() || self.state.armed || !self.connected;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(p.fg_strong)
                    .child("Shot delay (ms)"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        button("zero-delay-down", "−", ButtonKind::Secondary, p)
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(|controls, _, _, cx| {
                                controls.step_shot_delay(false, cx);
                            })),
                    )
                    .child(
                        div()
                            .w(px(96.))
                            .child(Input::new(&self.shot_delay_input).disabled(busy).h(px(40.))),
                    )
                    .child(
                        button("zero-delay-up", "+", ButtonKind::Secondary, p)
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(|controls, _, _, cx| {
                                controls.step_shot_delay(true, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        button("zero-delay-set", "Set delay", ButtonKind::Secondary, p)
                            .small()
                            .disabled(busy)
                            .on_click(
                                cx.listener(|controls, _, _, cx| controls.set_shot_delay(cx)),
                            ),
                    )
                    .child(
                        button("zero-delay-reset", "Reset delay", ButtonKind::Secondary, p)
                            .small()
                            .disabled(busy)
                            .on_click(
                                cx.listener(|controls, _, _, cx| controls.reset_shot_delay(cx)),
                            ),
                    )
                    .child(
                        button("zero-delay-save", "Save delay", ButtonKind::Secondary, p)
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(|controls, _, _, cx| {
                                controls.set_and_save_shot_delay(cx);
                            })),
                    ),
            )
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
