//! Moving between stages: back, retry, failure, exit.

use gpui::Context;

use super::{Stage, Wizard};
use crate::views::calibrate::automatic::pattern_window::PatternMode;
use crate::views::calibrate::automatic::worker::{Command, Failure, FailureKind};

impl Wizard {
    pub(crate) fn set_stage(&mut self, stage: Stage, cx: &mut Context<Self>) {
        self.stage = stage;
        if stage != Stage::Setup {
            self.cancel_helper(cx);
            self.camera_poll = None;
        } else if self.camera_poll.is_none() {
            self.camera_poll = Some(Self::poll_cameras(cx));
        }
        if stage != Stage::Problem {
            self.step = stage.step();
        }
        cx.notify();
    }

    /// Stops the camera and takes the pattern down.
    fn release_hardware(&mut self, cx: &mut Context<Self>) {
        self.cancel_flow();
        self.send(Command::Close, cx);
        self.close_pattern(cx);
        self.restore_window(cx);
        self.restarting = false;
    }

    pub(crate) fn back_to_setup(&mut self, cx: &mut Context<Self>) {
        self.release_hardware(cx);
        // Ignore anything the stopped session still reports.
        self.session += 1;
        self.preview = None;
        self.markers = None;
        self.finished = None;
        self.use_measured = false;
        self.saved_to = None;
        self.confirm_replace = false;
        self.saving = false;
        self.save_task = None;
        self.attempt_saving = false;
        self.attempt_status = None;
        self.save_error = None;
        self.set_stage(Stage::Setup, cx);
        self.refresh_saved_screens(cx);
    }

    /// Calibrate another screen: back to Setup with the screen number chosen afresh and
    /// the size cleared, since it described the last screen.
    pub(crate) fn calibrate_another(&mut self, cx: &mut Context<Self>) {
        self.screen_choice = None;
        self.size_text.clear();
        self.manual_size = false;
        self.back_to_setup(cx);
    }

    /// Leaves the wizard: the camera stops and the wizard's window closes.
    pub(crate) fn exit(&mut self, cx: &mut Context<Self>) {
        self.release_hardware(cx);
        let host = self.host;
        // Deferred: this runs inside an update of the window being removed.
        cx.defer(move |cx| {
            host.update(cx, |_, window, _| window.remove_window()).ok();
        });
    }

    /// Zero lives on Home, in the main window: close the wizard and show it.
    pub(crate) fn go_home(&mut self, cx: &mut Context<Self>) {
        self.exit(cx);
        cx.defer(crate::shell::show_home);
    }

    pub(super) fn problem(&mut self, failure: Failure, cx: &mut Context<Self>) {
        // A failed preview can arrive during the same-monitor countdown.
        // Stop it before it can move the problem screen back into Measuring.
        self.cancel_flow();
        self.close_pattern(cx);
        self.restore_window(cx);
        self.failure = Some(failure);
        self.set_stage(Stage::Problem, cx);
    }

    /// The button on the problem screen: back to the step that can try again.
    pub(crate) fn retry(&mut self, cx: &mut Context<Self>) {
        let Some(failure) = self.failure.clone() else {
            return self.back_to_setup(cx);
        };
        match failure.kind {
            FailureKind::Markers if failure.camera_open => {
                if !self.same_monitor {
                    self.open_pattern(PatternMode::Black, cx);
                }
                self.enter_markers(cx);
            }
            // The size lives in Setup; trying again would meet the same error.
            FailureKind::Internal | FailureKind::SizeInvalid | FailureKind::SizeShape => {
                self.back_to_setup(cx);
            }
            _ => self.begin_position(cx),
        }
    }
}
