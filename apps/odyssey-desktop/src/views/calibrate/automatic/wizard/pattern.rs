//! The pattern window the wizard puts on the target monitor.

use dioxus::prelude::*;
use odyssey_autocalib::pattern::Pattern;

use super::Wizard;
use crate::views::calibrate::automatic::pattern_window::{self, PatternMode};

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
}
