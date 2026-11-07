//! The pattern window the wizard puts on the target monitor.

use gpui::{App, Context};
use odyssey_autocalib::pattern::Pattern;

use super::Wizard;
use crate::views::calibrate::automatic::pattern_window::{
    self, PatternEvent, PatternLoss, PatternMode,
};

/// What to tell someone when the pattern window did not land on the monitor
/// they chose.
const WRONG_MONITOR_ADVICE: &str = "The picture didn't appear on the screen you chose. Your window manager moved it; allow the \"Calibration pattern\" window to open fullscreen on the chosen display and try again.";

impl Wizard {
    /// Whether a fullscreen picture on the selected monitor would cover (part
    /// of) the wizard's own window.
    pub(super) fn pattern_covers_host(&self, cx: &App) -> bool {
        let Some(display) = self
            .monitors
            .read(cx)
            .selected_display(cx)
            .and_then(|id| cx.find_display(id))
        else {
            return false;
        };
        pattern_window::covers_window(self.host_bounds, self.host_display, display.as_ref())
    }

    pub(super) fn open_pattern(&mut self, mode: PatternMode, cx: &mut Context<Self>) -> bool {
        let (Some(key), Some(display), Some(aspect)) = (
            self.selected.clone(),
            self.monitors.read(cx).selected_display(cx),
            self.aspect(),
        ) else {
            return false;
        };
        let Some(opened) = pattern_window::open(
            display,
            (key.width, key.height),
            Pattern::new(aspect),
            mode,
            self.pattern_loss.clone(),
            cx,
        ) else {
            return false;
        };
        if let Some(previous) = self.pattern.replace(opened) {
            previous.close(cx);
        }
        cx.notify();
        true
    }

    pub(super) fn close_pattern(&mut self, cx: &mut Context<Self>) {
        if let Some(pattern) = self.pattern.take() {
            pattern.close(cx);
            cx.notify();
        }
    }

    pub(super) fn show(&self, mode: PatternMode, cx: &mut App) {
        if let Some(display) = &self.pattern {
            display.show(mode, cx);
        }
    }

    /// Brings the wizard's window to the front.
    pub(super) fn focus_host(&self, cx: &mut Context<Self>) {
        let host = self.host;
        // Deferred: this may run inside an update of the host window.
        cx.defer(move |cx| {
            host.update(cx, |_, window, _| window.activate_window())
                .ok();
        });
    }

    /// Reacts to the pattern window: the card frame resized from the
    /// keyboard, or the window lost.
    ///
    /// Without the pattern window, or with it on the wrong monitor, there is
    /// nothing valid for the camera to look at, so the calibration stops.
    pub(super) fn on_pattern_event(&mut self, event: PatternEvent, cx: &mut Context<Self>) {
        let loss = match event {
            PatternEvent::Nudge(steps) => return self.nudge_card(steps, cx),
            PatternEvent::Lost(loss) => loss,
        };
        let helper_covers = self.helper.as_ref().map(|helper| helper.covers);
        match helper_covers {
            Some(covers) => self.on_helper_pattern_lost(covers, loss, cx),
            None => self.on_calibration_pattern_lost(loss, cx),
        }
    }

    /// The helper's picture went away. When it covered the wizard's window,
    /// closing it is the way back to the wizard.
    fn on_helper_pattern_lost(
        &mut self,
        covers_wizard: bool,
        loss: PatternLoss,
        cx: &mut Context<Self>,
    ) {
        if !covers_wizard {
            self.cancel_helper(cx);
            self.note("The measuring screen was closed.".into(), cx);
            return;
        }
        self.close_pattern(cx);
        self.focus_host(cx);
        if loss == PatternLoss::WrongMonitor {
            self.helper_problem = Some(WRONG_MONITOR_ADVICE.into());
        }
        cx.notify();
    }

    fn on_calibration_pattern_lost(&mut self, loss: PatternLoss, cx: &mut Context<Self>) {
        if self.pattern.is_none() {
            return;
        }
        tracing::info!("pattern window lost ({loss:?}); stopping the calibration");
        self.back_to_setup(cx);
        self.note(
            match loss {
                PatternLoss::Closed => {
                    "The pattern screen was closed, so the calibration stopped.".into()
                }
                PatternLoss::WrongMonitor => WRONG_MONITOR_ADVICE.into(),
            },
            cx,
        );
    }
}
