//! The camera thread's state and its command loop.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use dioxus::logger::tracing;
use odyssey_autocalib::camera::CameraError;
use odyssey_autocalib::session::{AttemptFrames, Session, SessionError};
use tokio::sync::mpsc::UnboundedSender;

use super::backend::Backend;
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

struct Live {
    session: Session,
    ready: Stability,
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
            let streaming = self.live.is_some() && self.mode != Mode::Hold;
            let command = if streaming {
                match receiver.try_recv() {
                    Ok(command) => Some(command),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                }
            } else {
                match receiver.recv() {
                    Ok(command) => Some(command),
                    Err(_) => return,
                }
            };
            match command {
                Some(command) => self.handle(command),
                None => self.stream_one(),
            }
        }
    }

    fn emit(&self, event: SessionEvent) {
        let _ = self.events.send(Event::Session {
            session: self.session,
            event,
        });
    }

    fn close(&mut self) {
        self.mode = Mode::Hold;
        if self.live.take().is_some() {
            tracing::info!("camera closed (calibration session {})", self.session);
            self.emit(SessionEvent::Closed);
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

    fn close_quietly(&mut self) {
        if self.live.take().is_some() {
            tracing::info!("camera closed (calibration session {})", self.session);
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::ListCameras => {
                let result = self.backend.list();
                let _ = self.events.send(Event::Cameras(result));
            }
            Command::Open { session, request } => {
                self.close_quietly();
                self.session = session;
                self.timeouts = 0;
                self.size_origin = request.size_origin;
                self.require_edges = request.options.require_visible_edges;
                self.request_text = report::describe_request(&request);
                self.attempt = None;
                match self.backend.open(&request) {
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
            Command::SetMode(mode) => {
                if let Some(live) = &mut self.live {
                    live.ready.reset();
                    self.mode = mode;
                    self.timeouts = 0;
                }
            }
            Command::SetIrExposure(us) => {
                if let Some(live) = &mut self.live
                    && let Err(e) = live.session.set_ir_exposure_us(us)
                {
                    self.fail(FailureKind::CameraLost, e.to_string(), false);
                }
            }
            Command::MeasureScreen => self.measure_screen(),
            Command::MeasureMarkers => self.measure_markers(),
            Command::Finish => self.finish(),
            Command::SaveAttempt { path } => self.save_attempt(path),
            Command::Close => self.close(),
        }
    }

    fn measure_screen(&mut self) {
        let Some(live) = &mut self.live else { return };
        self.mode = Mode::Hold;
        let (events, session) = (&self.events, self.session);
        let result = live.session.measure_screen(&mut |step| {
            let _ = events.send(Event::Session {
                session,
                event: SessionEvent::Progress(step),
            });
        });
        match result {
            Ok(_) => self.emit(SessionEvent::ScreenMeasured),
            Err(e) => self.session_failed(e),
        }
    }

    fn measure_markers(&mut self) {
        let Some(live) = &mut self.live else { return };
        self.mode = Mode::Hold;
        let (events, session) = (&self.events, self.session);
        let result = live.session.measure_markers(&mut |step| {
            let _ = events.send(Event::Session {
                session,
                event: SessionEvent::Progress(step),
            });
        });
        match result {
            Ok(_) => self.emit(SessionEvent::MarkersMeasured),
            Err(e) => self.session_failed(e),
        }
    }

    fn session_failed(&mut self, error: SessionError) {
        let kind = FailureKind::of_session(&error);
        // A measurement that found nothing usable leaves the camera fine; a
        // camera error means the stream is gone.
        let keep = kind != FailureKind::CameraLost;
        self.fail(kind, error.to_string(), keep);
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
            Some(attempt) => {
                let log = crate::paths::log_dir()
                    .map(|dir| dir.join("latest.log"))
                    .and_then(|file| std::fs::read_to_string(file).ok())
                    .unwrap_or_default();
                let summary = report::attempt_summary(attempt);
                attempt_bundle::write(
                    &path,
                    &Bundle {
                        summary: &summary,
                        frames: attempt.frames.as_ref(),
                        log: &log,
                    },
                )
                .map(|bytes| SavedAttempt { path, bytes })
            }
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

    fn finish(&mut self) {
        let Some(live) = &self.live else { return };
        let result = live.session.finish().and_then(|declared| {
            // With a declared size, also solve the same frames without it, so
            // the review can offer the camera's own number after the camera
            // has stopped.
            let measured = if declared.review.size_check.is_some() {
                Some(live.session.finish_measured()?)
            } else {
                None
            };
            Ok((declared, measured))
        });
        match result {
            Ok((declared, measured)) => {
                let (image, width, height) = encode::encode_rgb(
                    &declared.review.color,
                    u32::MAX,
                    encode::REVIEW_JPEG_QUALITY,
                );
                // Kept for the user to save after the camera closes just below.
                let mut summary = format!("finished\n{}", report::result_text(&declared));
                if let Some(measured) = &measured {
                    summary.push_str(&format!(
                        "\nthe same frames solved with the declared size set aside (the camera's own size):\n{}",
                        report::result_text(measured)
                    ));
                }
                self.remember_attempt(summary);
                let finished = Finished {
                    image,
                    width,
                    height,
                    size_origin: self.size_origin,
                    primary: frame::outcome(declared),
                    measured: measured.map(frame::outcome),
                };
                self.emit(SessionEvent::Finished(Box::new(finished)));
                // The measurement is complete; the camera has nothing left to do.
                self.close();
            }
            Err(e) => self.session_failed(e),
        }
    }

    /// Grabs and processes one frame in the current mode.
    fn stream_one(&mut self) {
        let Some(live) = &mut self.live else { return };
        let now = Instant::now();
        match self.mode {
            Mode::Hold => {}
            Mode::Preview => match live.session.preview() {
                Ok(preview) => {
                    self.timeouts = 0;
                    let ready_for = live.ready.observe(preview.is_ready(), now);
                    if self.claim_frame() {
                        self.emit(SessionEvent::Preview(Box::new(frame::preview_frame(
                            preview,
                            ready_for,
                            self.require_edges,
                        ))));
                    }
                }
                Err(e) => self.stream_error(e),
            },
            Mode::Markers => match live.session.preview_markers() {
                Ok(preview) => {
                    self.timeouts = 0;
                    if self.claim_frame() {
                        self.emit(SessionEvent::Markers(Box::new(frame::marker_frame(
                            preview,
                        ))));
                    }
                }
                Err(e) => self.stream_error(e),
            },
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
        if matches!(error, SessionError::Camera(CameraError::Timeout)) {
            self.timeouts += 1;
            if self.timeouts < MAX_CONSECUTIVE_TIMEOUTS {
                return;
            }
        }
        self.fail(FailureKind::of_session(&error), error.to_string(), false);
    }
}
