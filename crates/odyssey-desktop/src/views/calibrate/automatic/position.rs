//! Step 2: aim the camera. Live picture with detection overlays, one plain
//! sentence about what to change, and a brightness slider.

use std::sync::Arc;
use std::time::Duration;

use gpui::component::slider::{SliderEvent, SliderState, SliderValue};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div,
};
use odyssey_autocalib::session::Hint;

use super::hints::{OVERSCAN_HELP, screen_hint};
use super::settings::{COLOR_EXPOSURE_RANGE_US, COLOR_EXPOSURE_STEP_US};
use super::ui::{act, camera_placeholder, details, fact, open_state, step_page};
use super::wizard::Wizard;
use super::worker::PreviewFrame;
use crate::components::ui::{ButtonKind, button, card};
use crate::components::wizard::{FrameSlot, Icon, Tone, overlay_image, slider, status_banner};
use crate::theme::{Palette, palette};

/// How long the brightness slider rests before the stream restarts with it.
const EXPOSURE_DEBOUNCE: Duration = Duration::from_millis(450);

/// Brightness slider position as a percentage of its range.
fn exposure_percent(us: u32) -> u32 {
    let (lo, hi) = (
        *COLOR_EXPOSURE_RANGE_US.start(),
        *COLOR_EXPOSURE_RANGE_US.end(),
    );
    (us.clamp(lo, hi) - lo) * 100 / (hi - lo)
}

pub(super) struct PositionStep {
    wizard: Entity<Wizard>,
    exposure: Entity<SliderState>,
    /// Bumped on every slider movement; a debounce that finds it changed gives up.
    generation: u64,
    frame: FrameSlot,
    _subscriptions: Vec<Subscription>,
}

impl PositionStep {
    pub(super) fn new(wizard: Entity<Wizard>, cx: &mut Context<Self>) -> Self {
        let exposure = cx.new(|_| {
            SliderState::new()
                .max(*COLOR_EXPOSURE_RANGE_US.end() as f32)
                .min(*COLOR_EXPOSURE_RANGE_US.start() as f32)
                .step(COLOR_EXPOSURE_STEP_US as f32)
        });
        let subscriptions = vec![
            cx.observe(&wizard, |_, _, cx| cx.notify()),
            cx.subscribe(&exposure, |this, _, event: &SliderEvent, cx| {
                let SliderEvent::Change(SliderValue::Single(value)) = event else {
                    return;
                };
                let us = value.round().max(0.0) as u32;
                this.wizard
                    .update(cx, |wizard, cx| wizard.set_color_exposure(us, cx));
                this.generation += 1;
                let mine = this.generation;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(EXPOSURE_DEBOUNCE).await;
                    this.update(cx, |this, cx| {
                        // A later movement of the slider supersedes this one.
                        if this.generation == mine {
                            this.wizard.update(cx, Wizard::restart_camera);
                        }
                    })
                    .ok();
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

impl Render for PositionStep {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let wizard = self.wizard.clone();
        let details_open = open_state("position-details", window, cx);
        let target = SliderValue::Single(wizard.read(cx).color_exposure as f32);
        if self.exposure.read(cx).value() != target {
            self.exposure
                .update(cx, |slider, cx| slider.set_value(target, window, cx));
        }
        let state = wizard.read(cx);
        let same_monitor = state.same_monitor;
        let ready = state.position_ready();
        let has_frame = state.preview.is_some();
        let exposure = state.color_exposure;
        let restarting = state.restarting;
        let camera = state.camera().map(|c| c.name).unwrap_or_default();
        self.frame
            .show(state.preview.as_ref().map(|f| &f.image), window);

        let subtitle = if same_monitor {
            "Aim the camera at the screen, then press Start. The screen counts down and then shows the pattern, over this window, while it is measured."
        } else {
            "Hold the camera in front of the screen so the whole dot pattern is in view."
        };

        let primary = if same_monitor {
            button("start", "Start", ButtonKind::Primary, p)
                .disabled(!has_frame)
                .on_click(act(&wizard, Wizard::start_countdown))
        } else {
            button("next", "Next", ButtonKind::Primary, p)
                .disabled(!ready)
                .on_click(act(&wizard, Wizard::measure_screen))
        };
        let footer = vec![
            button("back", "Back", ButtonKind::Secondary, p)
                .on_click(act(&wizard, Wizard::back_to_setup))
                .into_any_element(),
            primary.into_any_element(),
        ];

        let value_text = if restarting {
            "Applying…".to_string()
        } else {
            format!("{}%", exposure_percent(exposure))
        };
        let content = vec![
            banner(p, state),
            live_preview(p, state),
            card(p)
                .p_4()
                .child(slider(
                    p,
                    &self.exposure,
                    "Brightness",
                    value_text,
                    Some("If the dots look washed out or too faint, adjust this until they are crisp white dots on black."),
                ))
                .into_any_element(),
            position_details(p, state, camera, exposure, &details_open, cx),
        ];

        step_page(
            p,
            "Point the camera at the screen",
            Some(subtitle.into()),
            content,
            footer,
        )
    }
}

fn banner(p: Palette, wizard: &Wizard) -> AnyElement {
    let preview = wizard.preview.as_ref();
    if wizard.same_monitor {
        return status_banner(
            p,
            if preview.is_some() { Tone::Good } else { Tone::Attention },
            if preview.is_some() { Icon::Camera } else { Icon::Viewfinder },
            if preview.is_some() {
                "Aim the camera at the screen"
            } else {
                "Starting the camera…"
            },
            "The dot pattern appears after a short countdown, so there is nothing to line up yet. Make sure the whole screen is in the picture.",
        )
        .into_any_element();
    }

    let Some(frame) = preview else {
        return status_banner(
            p,
            Tone::Attention,
            Icon::Camera,
            "Starting the camera…",
            "This takes a few seconds.",
        )
        .into_any_element();
    };

    let message = if frame.hint == Hint::Ready && !wizard.position_ready() {
        // Ready for a moment is not ready yet; asking for a still hand avoids a
        // measurement started on a passing glimpse.
        let mut hold = screen_hint(Hint::ScreenNotFound);
        hold.icon = Icon::Camera;
        hold.title = "Almost there — hold it steady".into();
        hold.detail = "Keep the camera still for a moment.".into();
        hold
    } else {
        screen_hint(frame.hint)
    };
    status_banner(p, message.tone, message.icon, message.title, message.detail).into_any_element()
}

fn live_preview(p: Palette, wizard: &Wizard) -> AnyElement {
    let Some(frame) = wizard.preview.as_ref() else {
        return camera_placeholder(p, "Waiting for the camera…").into_any_element();
    };
    let color = if frame.hint == Hint::Ready {
        p.success
    } else {
        p.warning
    };
    let unit = frame.width as f32 / 640.0;
    let dot_radius = 6.0 * unit;
    let line = 3.0 * unit;
    let quad = frame.quad;
    // Edges the display cuts off, drawn over the outline in red so the tech
    // sees which sides to look at. The quad runs top left, top right, bottom
    // right, bottom left.
    let cropped_edges: Vec<[[f32; 2]; 2]> = match (frame.quad, frame.cropped) {
        (Some(q), Some(sides)) => [
            (sides.top, 0),
            (sides.right, 1),
            (sides.bottom, 2),
            (sides.left, 3),
        ]
        .into_iter()
        .filter(|&(is_cropped, _)| is_cropped)
        .map(|(_, i)| [q[i], q[(i + 1) % 4]])
        .collect(),
        _ => Vec::new(),
    };
    let dots = frame.dots.clone();
    let danger = p.danger;
    overlay_image(
        Some(Arc::clone(&frame.image)),
        frame.width,
        frame.height,
        move |canvas, window| {
            if let Some(quad) = quad {
                canvas.stroke_path(window, &quad, true, line, color);
            }
            for edge in &cropped_edges {
                canvas.stroke_path(window, edge, false, line * 2.2, danger);
            }
            for dot in &dots {
                canvas.stroke_circle(window, *dot, dot_radius, line * 0.7, color);
            }
        },
        Vec::new(),
    )
    .into_any_element()
}

fn depth_text(frame: &PreviewFrame) -> (String, String) {
    (
        frame.distance_m.map_or("—".into(), |d| format!("{d:.2} m")),
        frame
            .depth_coverage
            .map_or("—".into(), |c| format!("{:.0} %", c * 100.0)),
    )
}

fn position_details(
    p: Palette,
    wizard: &Wizard,
    camera: String,
    exposure: u32,
    open: &Entity<bool>,
    cx: &gpui::App,
) -> AnyElement {
    let preview = wizard.preview.as_ref();
    let mut body = vec![fact(p, "Camera", camera)];
    if let Some(frame) = preview {
        let (distance, coverage) = depth_text(frame);
        body.push(fact(p, "Distance to screen", distance));
        body.push(fact(p, "Depth coverage on screen", coverage));
        body.push(fact(
            p,
            "Dots found",
            format!("{} of {}", frame.dots_matched, frame.dots_expected),
        ));
        body.push(fact(
            p,
            "Camera tilt",
            frame.tilt_deg.map_or("—".into(), |(pitch, roll)| {
                format!("pitch {pitch:.1}°, roll {roll:.1}°")
            }),
        ));
        body.push(fact(
            p,
            "Picture size",
            format!("{} × {}", frame.width, frame.height),
        ));
    }
    body.push(fact(p, "Color exposure", format!("{exposure} µs")));
    if preview.is_some_and(|f| f.hint == Hint::EdgesCropped) {
        let mut help =
            div()
                .mt_1()
                .flex()
                .flex_col()
                .gap_2()
                .border_t_1()
                .border_color(p.line)
                .pt_3()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(p.fg_strong)
                        .child("Where to turn off overscan"),
                )
                .child(div().text_color(p.fg_muted).child(
                    "Menu names differ between models and years. These are the usual places:",
                ));
        for (brand, steps) in OVERSCAN_HELP {
            help = help.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("{brand}:")),
                    )
                    .child(steps),
            );
        }
        body.push(help.into_any_element());
    }
    details("position-details", p, "Details", body, open, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_percent_spans_the_slider() {
        assert_eq!(exposure_percent(*COLOR_EXPOSURE_RANGE_US.start()), 0);
        assert_eq!(exposure_percent(*COLOR_EXPOSURE_RANGE_US.end()), 100);
        assert_eq!(exposure_percent(0), 0);
        assert_eq!(exposure_percent(u32::MAX), 100);
    }
}
