//! Events from the camera thread.

use std::rc::Rc;

use dioxus::prelude::*;
use odyssey_autocalib::session::Step;

use super::{CameraList, Stage, Wizard};
use crate::views::calibrate::automatic::worker::{Command, Event, ListError, SessionEvent};

impl Wizard {
    pub(super) fn on_event(mut self, event: Event) {
        match event {
            Event::Cameras(result) => {
                *self.list_pending.write() = false;
                *self.cameras.write() = match result {
                    Ok(list) => CameraList::Found(list),
                    Err(ListError::Runtime(status)) => CameraList::Unavailable(status),
                    Err(ListError::Camera(e)) => CameraList::Failed(e),
                };
                self.maybe_auto_start();
            }
            Event::Session { session, event } => {
                // A frame must be acknowledged even when its session is gone,
                // or the camera thread waits forever to send the next one.
                if matches!(event, SessionEvent::Preview(_) | SessionEvent::Markers(_)) {
                    self.worker.peek().frame_consumed();
                }
                if session != *self.session.peek() {
                    return;
                }
                self.on_session_event(event);
            }
        }
    }

    fn on_session_event(mut self, event: SessionEvent) {
        match event {
            SessionEvent::Opened => *self.restarting.write() = false,
            SessionEvent::Preview(frame) => {
                *self.restarting.write() = false;
                *self.preview.write() = Some(*frame);
            }
            SessionEvent::Markers(frame) => *self.markers.write() = Some(*frame),
            SessionEvent::Progress(step) => *self.progress.write() = Some(step),
            SessionEvent::ScreenMeasured => {
                self.restore_window();
                self.enter_markers();
            }
            SessionEvent::MarkersMeasured => {
                *self.progress.write() = Some(Step::Solving);
                self.send(Command::Finish);
            }
            SessionEvent::Finished(finished) => {
                *self.finished.write() = Some(Rc::new(*finished));
                *self.use_measured.write() = false;
                *self.saved_to.write() = None;
                *self.confirm_replace.write() = false;
                *self.save_error.write() = None;
                *self.attempt_status.write() = None;
                *self.attempt_saving.write() = false;
                self.close_pattern();
                self.restore_window();
                self.set_stage(Stage::Review);
            }
            SessionEvent::Failed(failure) => {
                *self.attempt_status.write() = None;
                *self.attempt_saving.write() = false;
                self.problem(failure);
            }
            SessionEvent::Closed => {}
            SessionEvent::AttemptSaved(result) => {
                *self.attempt_saving.write() = false;
                *self.attempt_status.write() = Some(result);
            }
        }
    }

    /// Skips Setup when the command line already answered it: a monitor was
    /// named and a camera is known. Waits for the hub server to have started,
    /// because a second app instance exits right after launch and must not
    /// open the pattern window on its way out.
    pub(super) fn maybe_auto_start(mut self) {
        if !*self.auto_start.peek()
            || !*self.setup_ready.peek()
            || *self.stage.peek() != Stage::Setup
        {
            return;
        }
        if self.server_ready.is_some_and(|ready| !*ready.0.peek()) {
            return;
        }
        if matches!(&*self.cameras.peek(), CameraList::Searching) {
            return;
        }
        if self.camera().is_some() && self.can_continue() {
            *self.auto_start.write() = false;
            self.begin_position();
        } else if matches!(&*self.cameras.peek(), CameraList::Found(list) if !list.is_empty())
            && self.camera_choice.peek().is_some()
        {
            // The named camera is not attached; let the tech choose.
            *self.auto_start.write() = false;
            let name = self.camera_choice.peek().clone().unwrap_or_default();
            self.note(format!(
                "Camera '{name}' isn't connected, so pick one below."
            ));
            *self.camera_choice.write() = None;
        }
    }
}
