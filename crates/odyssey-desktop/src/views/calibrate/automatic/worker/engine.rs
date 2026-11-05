//! The camera thread's state and its command loop.
//!
//! The thread is a small state machine. With no `live` session it is idle and
//! blocks on the command channel. With a session open, `mode` says how the
//! time between commands is spent: `Hold` keeps blocking on the channel,
//! `Preview` and `Markers` grab and report one frame, then look at the channel
//! again. Every command runs to completion before the next is read.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvError, TryRecvError};
use std::time::Instant;

use odyssey_autocalib::camera::CameraError;
use odyssey_autocalib::session::{AttemptFrames, Calibration, Session, SessionError, Step};
use tokio::sync::mpsc::UnboundedSender;

use super::backend::{Backend, ListError, OpenRequest};
use super::command::{Command, Mode};
use super::event::{Event, SavedAttempt, SessionEvent};
use super::failure::{Failure, FailureKind};
use super::frame::{self, Finished};
use super::stability::Stability;
use super::{encode, report};
use crate::views::calibrate::automatic::attempt_bundle::{self, Bundle};
use crate::views::calibrate::automatic::kind::SizeOrigin;

/// Consecutive missed frames tolerated before the camera counts as lost.
const MAX_CONSECUTIVE_TIMEOUTS: u32 = 3;

/// What the camera thread remembers of the latest attempt.
pub(super) struct Attempt {
    pub(super) request: String,
    pub(super) outcome: String,
    /// `None` when no camera was open, e.g. when it would not start.
    pub(super) frames: Option<AttemptFrames>,
}

/// An open camera session and how long its preview has read "ready".
struct Live {
    session: Session,
    ready: Stability,
}

/// What the loop does next.
enum Turn {
    Command(Command),
    /// No command is waiting and the camera is streaming: serve one frame.
    Frame,
    /// The [`super::Worker`] is gone; nobody is left to report to.
    Stop,
}

pub(super) struct Engine {
    backend: Backend,
    events: UnboundedSender<Event>,
    in_flight: Arc<AtomicBool>,
    live: Option<Live>,
    mode: Mode,
    session: u64,
    timeouts: u32,
    /// Origin of the declared size of the open session.
    size_origin: Option<SizeOrigin>,
    /// Whether the open session requires the display's whole picture.
    require_edges: bool,
    /// The latest attempt, kept after its camera closes so it can be saved.
    attempt: Option<Attempt>,
    /// The open request of the current session, as words.
    request_text: String,
}

fn send_session_event(events: &UnboundedSender<Event>, session: u64, event: SessionEvent) {
    // A send fails only once the UI is gone, and then nobody needs the event.
    let _ = events.send(Event::Session { session, event });
}

impl Engine {
    pub(super) fn new(
        backend: Backend,
        events: UnboundedSender<Event>,
        in_flight: Arc<AtomicBool>,
    ) -> Self {
        Self {
            backend,
            events,
            in_flight,
            live: None,
            mode: Mode::Hold,
            session: 0,
            timeouts: 0,
            size_origin: None,
            require_edges: false,
            attempt: None,
            request_text: String::new(),
        }
    }

    pub(super) fn run(mut self, receiver: Receiver<Command>) {
        let outcome = catch_unwind(AssertUnwindSafe(|| self.serve(&receiver)));
        if outcome.is_err() {
            tracing::error!("camera thread panicked");
            // A listing has no session ID. Complete it even if the UI has
            // already abandoned the last session, or Setup would keep waiting.
            let _ = self.events.send(Event::Cameras(Err(ListError::Camera(
                "the camera thread stopped unexpectedly".into(),
            ))));
            self.emit(SessionEvent::Failed(Failure {
                kind: FailureKind::Internal,
                details: "the camera thread stopped unexpectedly".into(),
                camera_open: false,
                size_origin: self.size_origin,
            }));
        }
        // Unwinding or not, the session drops here and releases the camera.
        self.close();
    }

    fn serve(&mut self, receiver: &Receiver<Command>) {
        loop {
            match self.next_turn(receiver) {
                Turn::Command(command) => self.handle(command),
                Turn::Frame => self.stream_one(),
                Turn::Stop => return,
            }
        }
    }

    fn next_turn(&self, receiver: &Receiver<Command>) -> Turn {
        let streaming = self.live.is_some() && self.mode != Mode::Hold;
        if streaming {
            match receiver.try_recv() {
                Ok(command) => Turn::Command(command),
                Err(TryRecvError::Empty) => Turn::Frame,
                Err(TryRecvError::Disconnected) => Turn::Stop,
            }
        } else {
            match receiver.recv() {
                Ok(command) => Turn::Command(command),
                Err(RecvError) => Turn::Stop,
            }
        }
    }

    fn emit(&self, event: SessionEvent) {
        send_session_event(&self.events, self.session, event);
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::ListCameras => {
                let result = self.backend.list();
                let _ = self.events.send(Event::Cameras(result));
            }
            Command::Open { session, request } => self.open(session, &request),
            Command::SetMode(mode) => self.set_mode(mode),
            Command::SetIrExposure(us) => self.set_ir_exposure(us),
            Command::MeasureScreen => self.measure(
                |session, progress| session.measure_screen(progress).map(|_| ()),
                SessionEvent::ScreenMeasured,
            ),
            Command::MeasureMarkers => self.measure(
                |session, progress| session.measure_markers(progress).map(|_| ()),
                SessionEvent::MarkersMeasured,
            ),
            Command::Finish => self.finish(),
            Command::SaveAttempt { path } => self.save_attempt(path),
            Command::Close => self.close(),
        }
    }

    fn open(&mut self, session: u64, request: &OpenRequest) {
        self.close_quietly();
        self.session = session;
        self.timeouts = 0;
        self.size_origin = request.size_origin;
        self.require_edges = request.options.require_visible_edges;
        self.request_text = report::describe_request(request);
        self.attempt = None;
        match self.backend.open(request) {
            Ok(opened) => {
                tracing::info!(
                    "camera opened: {} (calibration session {session})",
                    request.camera.name
                );
                self.live = Some(Live {
                    session: opened,
                    ready: Stability::default(),
                });
                self.mode = Mode::Preview;
                self.emit(SessionEvent::Opened);
            }
            Err(e) => self.fail(FailureKind::CameraOpen, e.to_string(), false),
        }
    }

    /// Releases the camera and tells the UI, if one was open.
    fn close(&mut self) {
        self.mode = Mode::Hold;
        if self.close_quietly() {
            self.emit(SessionEvent::Closed);
        }
    }

    /// Releases the camera without an event; returns whether one was open.
    fn close_quietly(&mut self) -> bool {
        let was_open = self.live.take().is_some();
        if was_open {
            tracing::info!("camera closed (calibration session {})", self.session);
        }
        was_open
    }

    fn set_mode(&mut self, mode: Mode) {
        let Some(live) = &mut self.live else { return };
        live.ready.reset();
        self.mode = mode;
        self.timeouts = 0;
    }

    fn set_ir_exposure(&mut self, exposure_us: u32) {
        let Some(live) = &mut self.live else { return };
        if let Err(e) = live.session.set_ir_exposure_us(exposure_us) {
            self.fail(FailureKind::CameraLost, e.to_string(), false);
        }
    }

    fn fail(&mut self, kind: FailureKind, details: String, keep_camera: bool) {
        tracing::warn!("calibration failed ({kind:?}): {details}");
        self.remember_attempt(format!("failed ({kind:?}): {details}"));
        self.mode = Mode::Hold;
        if !keep_camera {
            self.close_quietly();
        }
        self.emit(SessionEvent::Failed(Failure {
            kind,
            details,
            camera_open: keep_camera,
            size_origin: self.size_origin,
        }));
    }

    fn session_failed(&mut self, error: SessionError) {
        let kind = FailureKind::of_session(&error);
        // A measurement that found nothing usable leaves the camera fine; a
        // camera error means the stream is gone.
        let keep = kind != FailureKind::CameraLost;
        self.fail(kind, error.to_string(), keep);
    }

    /// Runs one blocking measurement, reporting each of its steps as it goes,
    /// then `done` on success. The camera stops streaming while it measures.
    fn measure(
        &mut self,
        run: impl FnOnce(&mut Session, &mut dyn FnMut(Step)) -> Result<(), SessionError>,
        done: SessionEvent,
    ) {
        let Some(live) = &mut self.live else { return };
        self.mode = Mode::Hold;
        let (events, session) = (&self.events, self.session);
        let result = run(&mut live.session, &mut |step| {
            send_session_event(events, session, SessionEvent::Progress(step));
        });
        match result {
            Ok(()) => self.emit(done),
            Err(e) => self.session_failed(e),
        }
    }

    fn finish(&mut self) {
        let Some(live) = &self.live else { return };
        let solved = solve_declared_and_measured(&live.session);
        let (declared, measured) = match solved {
            Ok(solved) => solved,
            Err(e) => return self.session_failed(e),
        };
        let (image, width, height) = encode::encode_rgb(&declared.review.color, u32::MAX);
        // Kept so the user can save it after `close` below releases the camera.
        self.remember_attempt(report::finished_summary(&declared, measured.as_ref()));
        let finished = Finished {
            image,
            width,
            height,
            size_origin: self.size_origin,
            primary: frame::outcome(declared),
            measured: measured.map(frame::outcome),
        };
        self.emit(SessionEvent::Finished(Box::new(finished)));
        self.close();
    }

    /// Grabs and processes one frame in the current mode.
    fn stream_one(&mut self) {
        match self.mode {
            Mode::Hold => {}
            Mode::Preview => self.stream_preview(),
            Mode::Markers => self.stream_markers(),
        }
    }

    fn stream_preview(&mut self) {
        let Some(live) = &mut self.live else { return };
        let now = Instant::now();
        let preview = match live.session.preview() {
            Ok(preview) => preview,
            Err(e) => return self.stream_error(e),
        };
        self.timeouts = 0;
        let ready_for = live.ready.observe(preview.is_ready(), now);
        if self.claim_frame() {
            let frame = frame::preview_frame(preview, ready_for, self.require_edges);
            self.emit(SessionEvent::Preview(Box::new(frame)));
        }
    }

    fn stream_markers(&mut self) {
        let Some(live) = &mut self.live else { return };
        let preview = match live.session.preview_markers() {
            Ok(preview) => preview,
            Err(e) => return self.stream_error(e),
        };
        self.timeouts = 0;
        if self.claim_frame() {
            let frame = frame::marker_frame(preview);
            self.emit(SessionEvent::Markers(Box::new(frame)));
        }
    }

    /// Claims the right to send a frame. The UI releases it when it takes the
    /// frame, so a slow UI skips frames instead of queueing them.
    fn claim_frame(&self) -> bool {
        self.in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn stream_error(&mut self, error: SessionError) {
        // A missed frame interrupts the observed ready run even when the
        // camera recovers before the timeout limit closes it.
        if let Some(live) = &mut self.live {
            live.ready.reset();
        }
        if matches!(error, SessionError::Camera(CameraError::Timeout)) {
            self.timeouts += 1;
            if self.timeouts < MAX_CONSECUTIVE_TIMEOUTS {
                return;
            }
        }
        self.fail(FailureKind::of_session(&error), error.to_string(), false);
    }

    /// Keeps what the attempt looked at and how it ended, so the user can save it
    /// after the camera is closed. Replaced by the next attempt.
    fn remember_attempt(&mut self, outcome: String) {
        let frames = self
            .live
            .as_ref()
            .map(|live| live.session.attempt_frames())
            .or_else(|| self.attempt.take().and_then(|attempt| attempt.frames));
        self.attempt = Some(Attempt {
            request: self.request_text.clone(),
            outcome,
            frames,
        });
    }

    /// Writes the remembered attempt to `path`; here, not on the UI thread, because
    /// encoding the frames takes a moment.
    fn save_attempt(&mut self, path: PathBuf) {
        let result = match &self.attempt {
            None => Err("there is no calibration attempt to save yet".to_string()),
            Some(attempt) => write_bundle(attempt, path).map_err(|error| error.to_string()),
        };
        match &result {
            Ok(saved) => tracing::info!(
                "calibration attempt saved to {} ({} bytes)",
                saved.path.display(),
                saved.bytes
            ),
            Err(e) => tracing::warn!("could not save the calibration attempt: {e}"),
        }
        self.emit(SessionEvent::AttemptSaved(result));
    }
}

/// Solves the frames with the declared size and, when one was declared, again
/// without it, so the review can offer the camera's own number after the
/// camera has stopped.
fn solve_declared_and_measured(
    session: &Session,
) -> Result<(Calibration, Option<Calibration>), SessionError> {
    let declared = session.finish()?;
    let measured = if declared.review.size_check.is_some() {
        Some(session.finish_measured()?)
    } else {
        None
    };
    Ok((declared, measured))
}

fn write_bundle(attempt: &Attempt, path: PathBuf) -> eros::Result<SavedAttempt> {
    let log = crate::paths::log_dir()
        .map(|dir| dir.join("latest.log"))
        .and_then(|file| std::fs::read_to_string(file).ok())
        .unwrap_or_default();
    let summary = report::attempt_summary(attempt);
    let bundle = Bundle {
        summary: &summary,
        frames: attempt.frames.as_ref(),
        log: &log,
    };
    attempt_bundle::write(&path, &bundle).map(|bytes| SavedAttempt { path, bytes })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use odyssey_autocalib::camera::CaptureSettings;
    use odyssey_autocalib::session::{Hint, SessionOptions};

    use super::*;

    #[test]
    fn a_recovered_preview_must_reestablish_readiness_after_a_timeout() {
        let backend = Backend::Demo {
            overscan_fraction: 0.0,
        };
        let camera = backend.list().unwrap().remove(0);
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let mut engine = Engine::new(backend, events, Arc::new(AtomicBool::new(false)));
        engine.open(
            1,
            &OpenRequest {
                camera,
                settings: CaptureSettings::default(),
                aspect: 16.0 / 9.0,
                options: SessionOptions::default(),
                pattern_visible: true,
                size_origin: None,
            },
        );
        engine
            .live
            .as_mut()
            .unwrap()
            .ready
            .observe(true, Instant::now() - Duration::from_secs(1));
        engine.stream_error(SessionError::Camera(CameraError::Timeout));
        engine.stream_preview();

        assert!(matches!(
            received.try_recv().unwrap(),
            Event::Session {
                event: SessionEvent::Opened,
                ..
            }
        ));
        let Event::Session {
            event: SessionEvent::Preview(frame),
            ..
        } = received.try_recv().unwrap()
        else {
            panic!("the recovered camera must publish a preview");
        };
        assert_eq!(frame.hint, Hint::Ready);
        assert_eq!(frame.ready_for, Duration::ZERO);
    }
}
