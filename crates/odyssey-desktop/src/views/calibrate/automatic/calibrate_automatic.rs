//! The route: the wizard's frame, its header and the step that matches its stage.

use gpui::component::tooltip::Tooltip;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, Render, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};

use super::quality::{Verdict, assess};
use super::wizard::{Stage, Wizard};
use super::{markers, measuring, position, problem, review, setup};
use crate::components::ui::{ButtonKind, button};
use crate::components::wizard::{FrameSlot, stepper};
use crate::theme::{Palette, palette};

/// Open/closed state of the collapsibles on the stages drawn without a view of
/// their own.
struct Opens {
    problem: Entity<bool>,
    review: Entity<bool>,
    difference: Entity<bool>,
    saved: Entity<bool>,
}

pub(crate) struct CalibrateAutomaticView {
    wizard: Entity<Wizard>,
    focus: FocusHandle,
    setup: Entity<setup::SetupStep>,
    position: Entity<position::PositionStep>,
    markers: Entity<markers::MarkersStep>,
    opens: Opens,
    /// The stage the next Enter acts on is ready: the step's main button is
    /// "focused", so Enter does the obvious thing.
    armed: bool,
    shown_stage: Stage,
    review_frame: FrameSlot,
    _subscriptions: Vec<Subscription>,
}

impl CalibrateAutomaticView {
    pub(crate) fn new(wizard: Entity<Wizard>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            focus,
            setup: cx.new(|cx| setup::SetupStep::new(wizard.clone(), window, cx)),
            position: cx.new(|cx| position::PositionStep::new(wizard.clone(), cx)),
            markers: cx.new(|cx| markers::MarkersStep::new(wizard.clone(), cx)),
            opens: Opens {
                problem: cx.new(|_| false),
                review: cx.new(|_| false),
                difference: cx.new(|_| false),
                saved: cx.new(|_| false),
            },
            armed: false,
            shown_stage: Stage::Setup,
            review_frame: FrameSlot::default(),
            _subscriptions: vec![cx.observe(&wizard, |_, _, cx| cx.notify())],
            wizard,
        }
    }

    /// Whether the step's main button would be enabled and is the one Enter
    /// should press.
    fn primary_ready(&self, cx: &gpui::App) -> bool {
        let state = self.wizard.read(cx);
        match state.stage {
            Stage::Position if state.same_monitor => state.preview.is_some(),
            Stage::Position => state.position_ready(),
            Stage::Review => state.finished.as_ref().is_some_and(|f| {
                let verdict = assess(&f.outcome(state.use_measured).measures).verdict;
                matches!(verdict, Verdict::Good | Verdict::Poor)
            }),
            _ => false,
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            // Escape steps back, like the Back button on every step.
            "escape" => self.wizard.update(cx, |w, cx| {
                if w.stage != Stage::Saved {
                    w.back_to_setup(cx);
                }
            }),
            "enter" if self.armed => self.press_primary(cx),
            _ => {}
        }
    }

    fn press_primary(&self, cx: &mut Context<Self>) {
        if !self.primary_ready(cx) {
            return;
        }
        self.wizard.update(cx, |w, cx| match w.stage {
            Stage::Position if w.same_monitor => w.start_countdown(cx),
            Stage::Position => w.measure_screen(cx),
            Stage::Review => {
                let verdict = w
                    .finished
                    .as_ref()
                    .map(|f| assess(&f.outcome(w.use_measured).measures).verdict);
                if verdict == Some(Verdict::Poor) {
                    w.begin_position(cx);
                } else if !w.saving && !w.confirm_replace {
                    w.save(false, cx);
                }
            }
            _ => {}
        });
    }
}

impl Render for CalibrateAutomaticView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let wizard = self.wizard.clone();
        let (stage, step, demo, hidden) = {
            let state = wizard.read(cx);
            (state.stage, state.step, state.demo, state.is_hidden())
        };
        // The pattern covers the wizard while it shows.
        if hidden {
            return div().into_any_element();
        }

        let review_image = wizard.read(cx).finished.as_ref().map(|f| f.image.clone());
        self.review_frame.show(review_image.as_ref(), window);
        if stage != self.shown_stage {
            self.shown_stage = stage;
            self.armed = false;
        }
        // The main button becomes available while the tech holds the camera, so
        // Enter is the natural next move.
        if self.primary_ready(cx) && !self.armed {
            self.armed = true;
            window.focus(&self.focus, cx);
        }

        let stage_view = self.stage_view(p, stage, cx);
        div()
            .id("calibrate-automatic")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(p.canvas)
            .overflow_y_scroll()
            .child(
                div()
                    .mx_auto()
                    .w_full()
                    .max_w(px(768.0))
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap_6()
                    .px_6()
                    .py_6()
                    .child(header(p, &wizard, demo))
                    .child(stepper(p, &Stage::STEPS, step))
                    .child(stage_view),
            )
            .into_any_element()
    }
}

impl CalibrateAutomaticView {
    fn stage_view(&self, p: Palette, stage: Stage, cx: &mut Context<Self>) -> AnyElement {
        let wizard = &self.wizard;
        match stage {
            Stage::Setup => self.setup.clone().into_any_element(),
            Stage::Position => self.position.clone().into_any_element(),
            Stage::Measuring => measuring::measuring_step(
                p,
                wizard,
                wizard.read(cx),
                "Measuring the screen",
                "the screen",
                "Keep the camera still. This takes a few seconds.",
            )
            .into_any_element(),
            Stage::Markers => self.markers.clone().into_any_element(),
            Stage::MeasuringMarkers => measuring::measuring_step(
                p,
                wizard,
                wizard.read(cx),
                "Measuring the markers",
                "the markers",
                "Keep the camera still and the markers on.",
            )
            .into_any_element(),
            Stage::Review => {
                review::review_step(p, wizard, &self.opens.review, &self.opens.difference, cx)
                    .into_any_element()
            }
            Stage::Saved => review::saved_step(p, wizard, &self.opens.saved, cx).into_any_element(),
            Stage::Problem => {
                problem::problem_step(p, wizard, wizard.read(cx), &self.opens.problem, cx)
                    .into_any_element()
            }
        }
    }
}

fn header(p: Palette, wizard: &Entity<Wizard>, demo: bool) -> impl IntoElement {
    let mut title = div().flex().items_center().gap_3().child(
        div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(p.fg_muted)
            .child("CALIBRATE SCREEN · AUTOMATIC"),
    );
    if demo {
        title = title.child(
            div()
                .id("demo-badge")
                .rounded_full()
                .px(px(10.0))
                .py_0p5()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .whitespace_nowrap()
                .bg(p.accent_soft)
                .text_color(p.accent_ink)
                .child("Demo · simulated camera")
                .tooltip(|window, cx| {
                    Tooltip::new(
                        "A simulated camera and screen are used. Nothing real is measured.",
                    )
                    .build(window, cx)
                }),
        );
    }
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(title)
        .child(
            button("exit", "Back to setup", ButtonKind::Ghost, p)
                .small()
                .on_click(super::ui::act(wizard, |w, cx| w.exit(cx))),
        )
}
