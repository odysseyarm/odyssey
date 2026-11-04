//! Leaving stages: back, retry, failure, exit.

use dioxus::prelude::*;
use dioxus_router::navigator;

use super::{Stage, Wizard};
use crate::Route;
use crate::views::calibrate::automatic::pattern_window::PatternMode;
use crate::views::calibrate::automatic::worker::{Command, Failure, FailureKind};

impl Wizard {
    /// Stops the camera and takes the pattern down.
    fn release_hardware(mut self) {
        self.cancel_flow();
        self.send(Command::Close);
        self.close_pattern();
        self.restore_window();
        *self.restarting.write() = false;
    }

    pub(crate) fn back_to_setup(mut self) {
        self.release_hardware();
        // Ignore anything the stopped session still reports.
        *self.session.write() += 1;
        *self.preview.write() = None;
        *self.markers.write() = None;
        *self.finished.write() = None;
        *self.use_measured.write() = false;
        *self.saved_to.write() = None;
        *self.confirm_replace.write() = false;
        *self.save_error.write() = None;
        self.set_stage(Stage::Setup);
        spawn(async move { self.refresh_saved_screens().await });
    }

    /// Calibrate another screen: back to Setup with the screen number chosen afresh and
    /// the size cleared, since it described the last screen.
    pub(crate) fn calibrate_another(mut self) {
        *self.screen_choice.write() = None;
        self.size_text.write().clear();
        *self.manual_size.write() = false;
        self.back_to_setup();
    }

    /// Leaves the wizard for the Calibrate page.
    pub(crate) fn exit(self) {
        self.release_hardware();
        navigator().push(Route::Calibrate {});
    }

    /// Zero lives on Home.
    pub(crate) fn go_home(self) {
        navigator().push(Route::Home {});
    }

    pub(super) fn problem(mut self, failure: Failure) {
        self.close_pattern();
        self.restore_window();
        *self.failure.write() = Some(failure);
        self.set_stage(Stage::Problem);
    }

    /// The button on the problem screen: back to the step that can try again.
    pub(crate) fn retry(self) {
        let Some(failure) = self.failure.peek().clone() else {
            return self.back_to_setup();
        };
        match failure.kind {
            FailureKind::Markers if failure.camera_open => {
                if !*self.same_monitor.peek() {
                    self.open_pattern(PatternMode::Black);
                }
                self.enter_markers();
            }
            // The size lives in Setup; trying again would meet the same error.
            FailureKind::Internal | FailureKind::SizeInvalid | FailureKind::SizeShape => {
                self.back_to_setup();
            }
            _ => self.begin_position(),
        }
    }
}
