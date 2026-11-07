//! Events from the camera thread.

use std::rc::Rc;

use gpui::{App, Context};
use odyssey_autocalib::session::Step;

use super::{CameraList, Stage, Wizard};
use crate::hub::HubHandle;
use crate::views::calibrate::automatic::worker::{
    Command, Event, Failure, FailureKind, ListError, SessionEvent,
};

impl Wizard {
    pub(super) fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        let Err(error) = self.worker.send(command) else {
            return;
        };
        let details = "the camera thread is no longer running".to_string();
        match error.0 {
            Command::Close => {}
            Command::ListCameras => {
                self.on_event(Event::Cameras(Err(ListError::Camera(details))), cx);
            }
            Command::SaveAttempt { .. } => self.on_event(
                Event::Session {
                    session: self.session,
                    event: SessionEvent::AttemptSaved(Err(details)),
                },
                cx,
            ),
            _ => {
                self.restarting = false;
                self.problem(
                    Failure {
                        kind: FailureKind::Internal,
                        details,
                        camera_open: false,
                        size_origin: None,
                    },
                    cx,
                );
            }
        }
    }

    pub(crate) fn note(&mut self, text: String, cx: &mut Context<Self>) {
        self.notes.push(text);
        cx.notify();
    }

    pub(super) fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Cameras(result) => {
                self.list_pending = false;
                self.cameras = match result {
                    Ok(list) => CameraList::Found(list),
                    Err(ListError::Runtime(status)) => CameraList::Unavailable(status),
                    Err(ListError::Camera(e)) => CameraList::Failed(e),
                };
                cx.notify();
                self.maybe_auto_start(cx);
            }
            Event::Session { session, event } => {
                // A frame must be acknowledged even when its session is gone,
                // or the camera thread waits forever to send the next one.
                if matches!(event, SessionEvent::Preview(_) | SessionEvent::Markers(_)) {
                    self.worker.frame_consumed();
                }
                if session != self.session {
                    return;
                }
                self.on_session_event(event, cx);
            }
        }
    }

    fn on_session_event(&mut self, event: SessionEvent, cx: &mut Context<Self>) {
        match event {
            SessionEvent::Opened => self.restarting = false,
            SessionEvent::Preview(frame) => {
                self.restarting = false;
                self.preview = Some(*frame);
            }
            SessionEvent::Markers(frame) => self.markers = Some(*frame),
            SessionEvent::Progress(step) => self.progress = Some(step),
            SessionEvent::ScreenMeasured => {
                self.restore_window(cx);
                self.enter_markers(cx);
            }
            SessionEvent::MarkersMeasured => {
                self.progress = Some(Step::Solving);
                self.send(Command::Finish, cx);
            }
            SessionEvent::Finished(finished) => {
                self.finished = Some(Rc::new(*finished));
                self.use_measured = false;
                self.saved_to = None;
                self.confirm_replace = false;
                self.save_error = None;
                self.attempt_status = None;
                self.attempt_saving = false;
                self.close_pattern(cx);
                self.restore_window(cx);
                self.set_stage(Stage::Review, cx);
            }
            SessionEvent::Failed(failure) => {
                self.attempt_status = None;
                self.attempt_saving = false;
                self.problem(failure, cx);
            }
            SessionEvent::Closed => {}
            SessionEvent::AttemptSaved(result) => {
                self.attempt_saving = false;
                self.attempt_status = Some(result);
            }
        }
        cx.notify();
    }

    fn server_ready(cx: &App) -> bool {
        cx.global::<HubHandle>().0.read(cx).client.is_some()
    }

    /// Skips Setup when the command line already answered it: a monitor was
    /// named and a camera is known. Waits for the hub server to have started,
    /// because a second app instance exits right after launch and must not
    /// open the pattern window on its way out.
    pub(super) fn maybe_auto_start(&mut self, cx: &mut Context<Self>) {
        if !self.auto_start || !self.setup_ready || self.stage != Stage::Setup {
            return;
        }
        if !Self::server_ready(cx) {
            return;
        }
        if matches!(self.cameras, CameraList::Searching) {
            return;
        }
        if self.camera().is_some() && self.can_continue() {
            self.auto_start = false;
            self.begin_position(cx);
        } else if matches!(&self.cameras, CameraList::Found(list) if !list.is_empty())
            && self.camera_choice.is_some()
        {
            // The named camera is not attached; let the tech choose.
            self.auto_start = false;
            let name = self.camera_choice.take().unwrap_or_default();
            self.note(
                format!("Camera '{name}' isn't connected, so pick one below."),
                cx,
            );
        }
    }
}
