//! The marker stages and the exposure controls.

use gpui::Context;

use super::{Stage, Wizard};
use crate::views::calibrate::automatic::pattern_window::PatternMode;
use crate::views::calibrate::automatic::settings::{COLOR_EXPOSURE_RANGE_US, IR_EXPOSURE_RANGE_US};
use crate::views::calibrate::automatic::worker::{Command, Mode};

impl Wizard {
    pub(crate) fn measure_markers(&mut self, cx: &mut Context<Self>) {
        self.progress = None;
        self.set_stage(Stage::MeasuringMarkers, cx);
        self.send(Command::MeasureMarkers, cx);
    }

    pub(crate) fn set_ir_exposure(&mut self, us: u32, cx: &mut Context<Self>) {
        self.ir_exposure = us.clamp(*IR_EXPOSURE_RANGE_US.start(), *IR_EXPOSURE_RANGE_US.end());
        cx.notify();
    }

    /// Sends the chosen infrared exposure to the camera. Each change costs the
    /// camera a few frames to settle, so callers apply it once the slider rests.
    pub(crate) fn apply_ir_exposure(&mut self, cx: &mut Context<Self>) {
        self.send(Command::SetIrExposure(self.ir_exposure), cx);
    }

    pub(crate) fn set_color_exposure(&mut self, us: u32, cx: &mut Context<Self>) {
        self.color_exposure = us.clamp(
            *COLOR_EXPOSURE_RANGE_US.start(),
            *COLOR_EXPOSURE_RANGE_US.end(),
        );
        cx.notify();
    }

    pub(super) fn enter_markers(&mut self, cx: &mut Context<Self>) {
        self.markers = None;
        // The screen goes black so nothing but the markers is lit. When the
        // wizard shares the monitor, the pattern window steps away instead and
        // the wizard itself is what the monitor shows.
        if self.same_monitor {
            self.close_pattern(cx);
        } else {
            self.show(PatternMode::Black, cx);
        }
        self.set_stage(Stage::Markers, cx);
        self.send(Command::SetMode(Mode::Markers), cx);
    }
}
