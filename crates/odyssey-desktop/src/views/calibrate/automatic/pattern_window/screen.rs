//! The view inside the pattern window.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Context, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _,
    Render, Styled as _, Window, black, div,
};
use odyssey_autocalib::pattern::Pattern;
use tokio::sync::mpsc::UnboundedSender;

use super::mode::{CARD_BIG_STEPS, PatternEvent, PatternLoss, PatternMode};
use super::pictures::{Scale, card_picture, countdown, pattern_picture, ruler_picture};
use crate::theme;

pub(crate) struct PatternView {
    pattern: Rc<Pattern>,
    /// The display's size in the units the ruler and card are drawn in.
    pixels: (u32, u32),
    mode: PatternMode,
    events: UnboundedSender<PatternEvent>,
    /// Set by the wizard when it closes this window, so that is not reported.
    wizard_closed: Rc<Cell<bool>>,
    focus: FocusHandle,
}

impl PatternView {
    pub(super) fn new(
        pattern: Pattern,
        pixels: (u32, u32),
        mode: PatternMode,
        events: UnboundedSender<PatternEvent>,
        wizard_closed: Rc<Cell<bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cx.on_release(|this, _| {
            if !this.wizard_closed.get() {
                let _ = this.events.send(PatternEvent::Lost(PatternLoss::Closed));
            }
        })
        .detach();
        Self {
            pattern: Rc::new(pattern),
            pixels,
            mode,
            events,
            wizard_closed,
            focus,
        }
    }

    pub(super) fn set_mode(&mut self, mode: PatternMode) {
        self.mode = mode;
    }

    /// Escape closes the window. Enter does too, but only on a helper picture,
    /// where it is the way back to the wizard. The arrow and page keys resize the card.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, _: &mut Context<Self>) {
        let helper_picture = matches!(
            self.mode,
            PatternMode::Ruler { .. } | PatternMode::Card { .. }
        );
        let card = matches!(self.mode, PatternMode::Card { .. });
        let nudge = |steps: i32| {
            let _ = self.events.send(PatternEvent::Nudge(steps));
        };
        match event.keystroke.key.as_str() {
            "escape" => window.remove_window(),
            // Coming back from a helper picture. On the calibration pattern
            // Enter must not end it.
            "enter" if helper_picture => window.remove_window(),
            "up" | "right" if card => nudge(1),
            "down" | "left" if card => nudge(-1),
            "pageup" if card => nudge(CARD_BIG_STEPS),
            "pagedown" if card => nudge(-CARD_BIG_STEPS),
            _ => {}
        }
    }
}

impl Render for PatternView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let (screen_w, screen_h) = (f64::from(self.pixels.0), f64::from(self.pixels.1));
        let scale = Scale {
            width: screen_w,
            height: screen_h,
            x: f32::from(viewport.width) / self.pixels.0.max(1) as f32,
            y: f32::from(viewport.height) / self.pixels.1.max(1) as f32,
        };
        // The instructions' gray is the same in both themes and legible on black.
        let hint = theme::palette(cx).fg_faint;
        let picture = match self.mode {
            PatternMode::Pattern => pattern_picture(Rc::clone(&self.pattern)),
            PatternMode::Black => div().into_any_element(),
            PatternMode::Ruler {
                line_px,
                covers_wizard,
            } => ruler_picture(line_px, covers_wizard, scale, hint),
            PatternMode::Card {
                width_px,
                covers_wizard,
            } => card_picture(width_px, covers_wizard, scale, hint),
            PatternMode::Countdown(seconds) => countdown(seconds),
        };
        div()
            .id("pattern")
            .size_full()
            .bg(black())
            .overflow_hidden()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .child(picture)
    }
}
