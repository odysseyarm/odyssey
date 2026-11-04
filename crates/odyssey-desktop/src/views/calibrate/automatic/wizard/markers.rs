//! The marker stages and the exposure controls.

use dioxus::prelude::*;

use super::{Stage, Wizard};
use crate::views::calibrate::automatic::pattern_window::PatternMode;
use crate::views::calibrate::automatic::settings::{COLOR_EXPOSURE_RANGE_US, IR_EXPOSURE_RANGE_US};
use crate::views::calibrate::automatic::worker::{Command, Mode};

impl Wizard {
    pub(crate) fn measure_markers(mut self) {
        *self.progress.write() = None;
        self.set_stage(Stage::MeasuringMarkers);
        self.send(Command::MeasureMarkers);
    }

    pub(crate) fn set_ir_exposure(mut self, us: u32) {
        *self.ir_exposure.write() =
            us.clamp(*IR_EXPOSURE_RANGE_US.start(), *IR_EXPOSURE_RANGE_US.end());
    }

    /// Sends the chosen infrared exposure to the camera. Each change costs the
    /// camera a few frames to settle, so callers apply it once the slider rests.
    pub(crate) fn apply_ir_exposure(self) {
        self.send(Command::SetIrExposure(*self.ir_exposure.peek()));
    }

    pub(crate) fn set_color_exposure(mut self, us: u32) {
        *self.color_exposure.write() = us.clamp(
            *COLOR_EXPOSURE_RANGE_US.start(),
            *COLOR_EXPOSURE_RANGE_US.end(),
        );
    }

    pub(super) fn enter_markers(mut self) {
        *self.markers.write() = None;
        // The screen goes black so nothing but the markers is lit. When the
        // wizard shares the monitor, the pattern window steps away instead and
        // the wizard itself is what the monitor shows.
        if *self.same_monitor.peek() {
            self.close_pattern();
        } else {
            self.show(PatternMode::Black);
        }
        self.set_stage(Stage::Markers);
        self.send(Command::SetMode(Mode::Markers));
    }
}
