//! The pattern window the wizard puts on the target monitor.

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_autocalib::pattern::Pattern;

use super::Wizard;
use crate::views::calibrate::automatic::pattern_window::{
    self, PatternEvent, PatternLoss, PatternMode,
};

/// What to tell someone when the pattern window did not land on the monitor
/// they chose.
const WRONG_MONITOR_ADVICE: &str = "The picture didn't appear on the screen you chose. Your window manager moved it; allow the \"Calibration pattern\" window to open fullscreen on the chosen display and try again.";

impl Wizard {
    pub(super) fn open_pattern(mut self, mode: PatternMode) -> bool {
        let (Some(handle), Some(aspect)) = (self.monitors.selected_handle(), self.aspect()) else {
            return false;
        };
        let display = pattern_window::open(
            &dioxus::desktop::window(),
            handle,
            Pattern::new(aspect),
            mode,
            self.pattern_loss.peek().clone(),
        );
        *self.pattern.write() = Some(display);
        true
    }

    pub(super) fn close_pattern(mut self) {
        // Dropping the display closes its window.
        *self.pattern.write() = None;
    }

    pub(super) fn show(self, mode: PatternMode) {
        if let Some(display) = &*self.pattern.peek() {
            display.show(mode);
        }
    }

    /// Reacts to the pattern window's page: the card frame resized from the
    /// keyboard, or the window lost.
    ///
    /// Without the pattern window, or with it on the wrong monitor, there is
    /// nothing valid for the camera to look at, so the calibration stops.
    pub(super) fn on_pattern_event(self, event: PatternEvent) {
        let loss = match event {
            PatternEvent::Nudge(steps) => return self.nudge_card(steps),
            PatternEvent::Lost(loss) => loss,
        };
        let helper_covers = self.helper.peek().as_ref().map(|helper| helper.covers);
        match helper_covers {
            Some(covers) => self.on_helper_pattern_lost(covers, loss),
            None => self.on_calibration_pattern_lost(loss),
        }
    }

    /// The helper's picture went away. When it covered the wizard's window,
    /// closing it is how you come back.
    fn on_helper_pattern_lost(mut self, covers_wizard: bool, loss: PatternLoss) {
        if !covers_wizard {
            self.cancel_helper();
            self.note("The measuring screen was closed.".into());
            return;
        }
        self.close_pattern();
        dioxus::desktop::window().set_focus();
        if loss == PatternLoss::WrongMonitor {
            *self.helper_problem.write() = Some(WRONG_MONITOR_ADVICE.into());
        }
    }

    fn on_calibration_pattern_lost(self, loss: PatternLoss) {
        if self.pattern.peek().is_none() {
            return;
        }
        tracing::info!("pattern window lost ({loss:?}); stopping the calibration");
        self.back_to_setup();
        self.note(match loss {
            PatternLoss::Closed => {
                "The pattern screen was closed, so the calibration stopped.".into()
            }
            PatternLoss::WrongMonitor => WRONG_MONITOR_ADVICE.into(),
        });
    }
}
