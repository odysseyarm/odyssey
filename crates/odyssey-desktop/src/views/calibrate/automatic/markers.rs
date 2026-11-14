//! Step 4: the screen is black and the tech turns on the tracker markers. The
//! infrared view shows what the camera sees. Measuring starts when the tech presses
//! Measure, never by itself.

use std::sync::Arc;
use std::time::Duration;

use gpui::component::slider::{SliderEvent, SliderState, SliderValue};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div,
};

use super::hints::{MARKER_RULES, marker_hint};
use super::settings::{IR_EXPOSURE_RANGE_US, IR_EXPOSURE_STEP_US};
use super::ui::{act, camera_placeholder, details, fact, open_state, step_page};
use super::wizard::Wizard;
use crate::components::ui::{ButtonKind, button, card};
use crate::components::wizard::{
    FrameSlot, Icon, Tone, overlay_image, overlay_label, slider, status_banner,
};
use crate::theme::{Palette, palette};

/// How long the marker brightness slider rests before the camera follows it.
const EXPOSURE_DEBOUNCE: Duration = Duration::from_millis(200);

pub(super) struct MarkersStep {
    wizard: Entity<Wizard>,
    exposure: Entity<SliderState>,
    /// Bumped on every slider movement. A debounce that finds it changed gives up.
    generation: u64,
    frame: FrameSlot,
    _subscriptions: Vec<Subscription>,
}

impl MarkersStep {
    pub(super) fn new(wizard: Entity<Wizard>, cx: &mut Context<Self>) -> Self {
        let exposure = cx.new(|_| {
            SliderState::new()
                .max(*IR_EXPOSURE_RANGE_US.end() as f32)
                .min(*IR_EXPOSURE_RANGE_US.start() as f32)
                .step(IR_EXPOSURE_STEP_US as f32)
        });
        let subscriptions = vec![
            cx.observe(&wizard, |_, _, cx| cx.notify()),
            cx.subscribe(&exposure, |this, _, event: &SliderEvent, cx| {
                let SliderEvent::Change(SliderValue::Single(value)) = event else {
                    return;
                };
                let us = value.round().max(0.0) as u32;
                this.wizard
                    .update(cx, |wizard, cx| wizard.set_ir_exposure(us, cx));
                this.generation += 1;
                let mine = this.generation;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(EXPOSURE_DEBOUNCE).await;
                    let _ = this.update(cx, |this, cx| {
                        // A later movement of the slider supersedes this one.
                        if this.generation == mine {
                            this.wizard.update(cx, Wizard::apply_ir_exposure);
                        }
                    });
                })
                .detach();
            }),
        ];
        Self {
            wizard,
            exposure,
            generation: 0,
            frame: FrameSlot::default(),
            _subscriptions: subscriptions,
        }
    }
}

impl Render for MarkersStep {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &palette(cx);
        let wizard = self.wizard.clone();
        let details_open = open_state("marker-details", window, cx);
        let rules_open = open_state("marker-rules", window, cx);
        let target = SliderValue::Single(wizard.read(cx).ir_exposure as f32);
        if self.exposure.read(cx).value() != target {
            self.exposure
                .update(cx, |slider, cx| slider.set_value(target, window, cx));
        }
        let state = wizard.read(cx);
        self.frame
            .show(state.markers.as_ref().map(|f| &f.image), window);
        let ir = state.ir_exposure;
        let all_found = state.markers.as_ref().is_some_and(|f| f.matched.is_some());

        let footer = vec![
            div()
                .flex()
                .w_full()
                .flex_col()
                .gap_5()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            button("cancel", "Cancel", ButtonKind::Secondary, p)
                                .on_click(act(&wizard, Wizard::back_to_setup)),
                        )
                        .child(
                            button("measure", "Measure", ButtonKind::Primary, p)
                                .disabled(!all_found)
                                .on_click(act(&wizard, Wizard::measure_markers)),
                        ),
                )
                .child(marker_banner(p, state))
                .into_any_element(),
        ];

        let mut facts = Vec::new();
        if let Some(frame) = state.markers.as_ref() {
            facts.push(fact(p, "Bright spots seen", frame.blobs.len().to_string()));
            facts.push(fact(
                p,
                "Matched to the marker layout",
                if frame.matched.is_some() {
                    "6 of 6"
                } else {
                    "not all six"
                },
            ));
            facts.push(fact(
                p,
                "Picture size",
                format!("{} × {}", frame.width, frame.height),
            ));
        }
        facts.push(fact(p, "Infrared exposure", format!("{ir} µs")));

        let rules = MARKER_RULES
            .iter()
            .map(|(title, text)| {
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::MEDIUM).child(*title))
                    .child(div().text_color(p.fg_muted).child(*text))
                    .into_any_element()
            })
            .collect();

        let content = vec![
            marker_preview(p, state),
            card(p)
                .p_4()
                .child(slider(
                    p,
                    &self.exposure,
                    "Marker brightness",
                    format!("{ir} µs"),
                    Some("Only the six markers should show up as bright dots. Lower it if other lights appear, raise it if a marker is missing."),
                ))
                .into_any_element(),
            details("marker-details", p, "Details", facts, &details_open, cx).into_any_element(),
            details("marker-rules", p, "Where the markers can go", rules, &rules_open, cx)
                .into_any_element(),
        ];

        step_page(
            p,
            "Turn on the tracker markers",
            Some("The screen is dark now. Switch on all six markers and keep the camera where it is. Press Measure when you're ready.".into()),
            content,
            footer,
        )
    }
}

fn marker_banner(p: &Palette, wizard: &Wizard) -> AnyElement {
    let Some(frame) = wizard.markers.as_ref() else {
        return status_banner(
            p,
            Tone::Attention,
            Icon::Camera,
            "Switching the camera to marker view…",
            "This takes a moment.",
        )
        .into_any_element();
    };
    let message = marker_hint(frame.hint);
    status_banner(p, message.tone, message.icon, message.title, message.detail).into_any_element()
}

fn marker_preview(p: &Palette, wizard: &Wizard) -> AnyElement {
    let Some(frame) = wizard.markers.as_ref() else {
        return camera_placeholder(p, "Waiting for the camera…").into_any_element();
    };
    let unit = frame.width as f32 / 640.0;
    let radius = 11.0 * unit;
    let line = 2.5 * unit;
    let (success, warning) = (p.success, p.warning);
    // A blob matched to one of the six markers is green and numbered.
    let marker_of = |i: usize| frame.matched.and_then(|m| m.iter().position(|&b| b == i));
    let blobs = frame.blobs.clone();
    let matched: Vec<Option<usize>> = (0..blobs.len()).map(marker_of).collect();
    let labels = blobs
        .iter()
        .zip(&matched)
        .filter_map(|(blob, marker)| {
            marker.map(|marker| {
                overlay_label(
                    blob[0] + radius * 1.3,
                    blob[1] - radius * 1.1,
                    frame.width,
                    frame.height,
                    success,
                    (marker + 1).to_string(),
                )
            })
        })
        .collect();
    overlay_image(
        Some(Arc::clone(&frame.image)),
        frame.width,
        frame.height,
        move |canvas, window| {
            for (blob, marker) in blobs.iter().zip(&matched) {
                let color = if marker.is_some() { success } else { warning };
                canvas.stroke_circle(window, *blob, radius, line, color);
            }
        },
        labels,
    )
    .into_any_element()
}
