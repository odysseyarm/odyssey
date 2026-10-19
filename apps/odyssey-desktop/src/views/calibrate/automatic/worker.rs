//! The camera thread.
//!
//! A [`Session`] blocks on the camera for every frame, so one dedicated thread
//! owns it and talks to the UI through channels: [`Command`]s in, [`Event`]s
//! out. The thread also owns camera enumeration, which keeps every SDK call on
//! one thread and lets a listing finish before a stream opens.
//!
//! The camera streams only while a session is open, and a session closes on
//! [`Command::Close`], after [`Command::Finish`], on any camera error and when
//! the [`Worker`] is dropped. Nothing else keeps a stream alive.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use dioxus::logger::tracing;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, GrayImage, RgbImage};
use odyssey_autocalib::camera::{CameraBackend, CameraError, CameraInfo, CaptureSettings};
use odyssey_autocalib::markers::MarkerError;
use odyssey_autocalib::orbbec::{self, OrbbecBackend, RuntimeStatus};
use odyssey_autocalib::session::{
    AttemptFrames, Calibration, EdgeCheck, Hint, MarkerHint, MarkerPreview, Preview, Session,
    SessionError, SessionOptions, SideSet, SizeCheck, SizeSource, Step,
};
use odyssey_autocalib::solve::SolveError;
use odyssey_autocalib::synthetic::{SyntheticBackend, SyntheticScene};
use odyssey_common::ScreenCalibration;
use tokio::sync::mpsc::UnboundedSender;

use super::attempt_bundle::{self, Bundle};
use super::kind::SizeOrigin;
use super::quality::Measures;
use crate::sdk_install;

/// Widest picture sent to the UI while previewing. A webview paints this at
/// roughly its natural size, and the JPEG stays small enough for ~15 frames a
/// second as a data URL.
const PREVIEW_MAX_WIDTH: u32 = 800;
const PREVIEW_JPEG_QUALITY: u8 = 72;
const REVIEW_JPEG_QUALITY: u8 = 85;
/// Consecutive missed frames tolerated before the camera counts as lost.
const MAX_CONSECUTIVE_TIMEOUTS: u32 = 3;

/// Diagonal of the simulated screen, whatever size the tech declares: the
/// demo needs a truth that a wrong declaration can disagree with.
pub const DEMO_DIAGONAL_IN: f64 = 55.0;

/// Which cameras the worker offers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Backend {
    /// The simulated camera, and nothing else. `overscan_fraction` is how much
    /// the simulated TV crops its picture.
    Demo { overscan_fraction: f64 },
    /// Attached Orbbec cameras. Needs the Orbbec SDK installed on this machine;
    /// without it listing fails with [`ListError::Runtime`].
    Hardware,
}

/// Why a camera listing has no list to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListError {
    /// The camera library is missing, broken or of an unsupported version.
    Runtime(RuntimeStatus),
    /// The library is there but enumerating cameras failed.
    Camera(String),
}

fn list_hardware() -> Result<Vec<CameraInfo>, ListError> {
    // The installer's copy of the SDK, or one the user downloaded, is found here;
    // a copy elsewhere on the machine is still found by the loader on its own.
    let extra_dir = sdk_install::search_dir();
    match orbbec::runtime_status(extra_dir.as_deref()) {
        RuntimeStatus::Available { .. } => OrbbecBackend::with_extra_dir(extra_dir)
            .list()
            .map_err(|e| ListError::Camera(e.to_string())),
        unavailable => Err(ListError::Runtime(unavailable)),
    }
}

impl Backend {
    fn list(self) -> Result<Vec<CameraInfo>, ListError> {
        match self {
            Self::Demo { .. } => SyntheticBackend::default()
                .list()
                .map_err(|e| ListError::Camera(e.to_string())),
            Self::Hardware => list_hardware(),
        }
    }

    fn open(self, request: &OpenRequest) -> Result<Session, CameraError> {
        let stream = match self {
            Self::Demo { overscan_fraction } => {
                let scene = demo_scene(
                    request.aspect,
                    DEMO_DIAGONAL_IN,
                    request.pattern_visible,
                    overscan_fraction,
                );
                SyntheticBackend::new(scene).open(&request.camera, &request.settings)?
            }
            Self::Hardware => OrbbecBackend::with_extra_dir(sdk_install::search_dir())
                .open(&request.camera, &request.settings)?,
        };
        Ok(Session::start(
            stream,
            request.aspect,
            request.options.clone(),
        ))
    }
}

/// The simulated scene for a screen of the given shape. The synthetic camera
/// renders the same dot grid the wizard draws because both derive it from this
/// aspect ratio, and it sits farther from a larger screen so the screen fills
/// a similar part of the picture.
pub fn demo_scene(
    aspect: f64,
    diagonal_in: f64,
    pattern_visible: bool,
    overscan_fraction: f64,
) -> SyntheticScene {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        16.0 / 9.0
    };
    let diagonal_m = diagonal_in * 0.0254;
    let height = diagonal_m / (aspect * aspect + 1.0).sqrt();
    let mut scene = SyntheticScene {
        screen_size_m: [aspect * height, height],
        show_pattern: pattern_visible,
        overscan_fraction,
        // Deliver frames at the camera's rate instead of as fast as they render.
        realtime: true,
        ..SyntheticScene::default()
    };
    scene.screen_center_m.z = (2.0 * diagonal_in / DEMO_DIAGONAL_IN).clamp(0.9, 3.5);
    scene
}

/// What the UI asks the camera thread to open.
#[derive(Debug, Clone)]
pub struct OpenRequest {
    pub camera: CameraInfo,
    pub settings: CaptureSettings,
    /// Width over height of the screen showing the pattern.
    pub aspect: f64,
    pub options: SessionOptions,
    /// Whether the screen shows the pattern right now. Only the simulated
    /// camera cares: it draws what a real camera would see.
    pub pattern_visible: bool,
    /// Where the declared size in `options` came from, for the words of a
    /// failure and the review.
    pub size_origin: Option<SizeOrigin>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The stream is open but nothing reads it.
    Hold,
    Preview,
    Markers,
}

#[derive(Debug)]
pub enum Command {
    ListCameras,
    /// Closes any open session, opens the camera and starts previewing. Events
    /// that follow carry `session`.
    Open {
        session: u64,
        request: OpenRequest,
    },
    SetMode(Mode),
    SetIrExposure(u32),
    MeasureScreen,
    MeasureMarkers,
    /// Computes the result, then closes the camera.
    Finish,
    Close,
    /// Writes the latest attempt's frames, log lines and summary to `path`.
    SaveAttempt {
        path: PathBuf,
    },
}

#[derive(Debug)]
pub enum Event {
    Cameras(Result<Vec<CameraInfo>, ListError>),
    /// Something about the camera session `session`; the UI drops events of
    /// sessions it has already abandoned.
    Session {
        session: u64,
        event: SessionEvent,
    },
}

#[derive(Debug)]
pub enum SessionEvent {
    Opened,
    Preview(Box<PreviewFrame>),
    Markers(Box<MarkerFrame>),
    Progress(Step),
    ScreenMeasured,
    MarkersMeasured,
    Finished(Box<Finished>),
    Failed(Failure),
    /// The stream is stopped and the camera released.
    Closed,
    /// A diagnostic bundle was written, or could not be.
    AttemptSaved(Result<SavedAttempt, String>),
}

/// A saved diagnostic bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedAttempt {
    pub path: PathBuf,
    pub bytes: u64,
}

/// What the camera thread remembers of the latest attempt.
struct Attempt {
    request: String,
    outcome: String,
    /// `None` when no camera was open, e.g. when it would not start.
    frames: Option<AttemptFrames>,
}

/// One live color frame and what the detector found in it. Coordinates are
/// pixels of the original image (`width` by `height`), which is also the
/// overlay's coordinate system.
#[derive(Debug, Clone)]
pub struct PreviewFrame {
    pub image: String,
    pub width: u32,
    pub height: u32,
    pub dots: Vec<[f32; 2]>,
    pub quad: Option<[[f32; 2]; 4]>,
    pub dots_matched: usize,
    pub dots_expected: usize,
    pub depth_coverage: Option<f32>,
    pub distance_m: Option<f32>,
    pub tilt_deg: Option<(f64, f64)>,
    /// The sides of the picture the display crops. Set only when the wizard
    /// requires the whole picture, since a projector's crop does not matter.
    pub cropped: Option<SideSet>,
    pub hint: Hint,
    /// How long the hint has been `Ready` without interruption.
    pub ready_for: Duration,
}

#[derive(Debug, Clone)]
pub struct MarkerFrame {
    pub image: String,
    pub width: u32,
    pub height: u32,
    pub blobs: Vec<[f32; 2]>,
    /// Index into `blobs` for each of the six markers, when all were matched.
    pub matched: Option<[usize; 6]>,
    pub hint: MarkerHint,
    /// How long all six markers have been matched without interruption.
    pub stable_for: Duration,
}

/// One complete result: the calibration and what the review shows about it.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub calibration: ScreenCalibration<f32>,
    pub screen_quad: [[f32; 2]; 4],
    pub markers: [[f32; 2]; 6],
    pub size_m: [f64; 2],
    pub diagonal_in: f64,
    pub size_source: SizeSource,
    /// The depth measurement against the declared size; present when a size
    /// was declared.
    pub size_check: Option<SizeCheck>,
    pub measures: Measures,
    pub gravity_aligned: bool,
    /// Whether the display showed the whole picture. `Cropped` is only
    /// possible when the edges were not required, i.e. the camera sized it.
    pub edges: EdgeCheck,
}

/// What the review screen needs once the calibration is done.
#[derive(Debug, Clone)]
pub struct Finished {
    pub image: String,
    pub width: u32,
    pub height: u32,
    pub size_origin: Option<SizeOrigin>,
    /// The result with the declared size, or the camera's own when none was
    /// declared.
    pub primary: Outcome,
    /// The result with the declared size set aside, solved from the same
    /// frames. Present only when a size was declared.
    pub measured: Option<Outcome>,
}

impl Finished {
    pub fn outcome(&self, use_measured: bool) -> &Outcome {
        match (&self.measured, use_measured) {
            (Some(measured), true) => measured,
            _ => &self.primary,
        }
    }
}

/// What went wrong, coarsely enough to give advice; `details` keeps the
/// technical text for the Details section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub details: String,
    /// Whether the camera is still open and a retry can reuse it.
    pub camera_open: bool,
    /// Where the declared size came from, which changes what to tell the tech
    /// when it is refused.
    pub size_origin: Option<SizeOrigin>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The camera would not start.
    CameraOpen,
    /// The camera stopped delivering frames or reported an error.
    CameraLost,
    /// The pattern was not found while measuring the screen.
    ScreenDots,
    /// The depth data did not describe a flat screen.
    ScreenDepth,
    /// The measured screen cannot be right: mirrored, edge-on, or dots that do
    /// not agree with a flat panel.
    ScreenGeometry,
    /// The markers could not be found at their places.
    Markers,
    /// The display cuts off the picture's border on these sides.
    EdgesCropped(SideSet),
    /// The declared size is not a usable size.
    SizeInvalid,
    /// The declared size has a different shape than the picture.
    SizeShape,
    Internal,
}

impl FailureKind {
    fn of_solve(error: &SolveError) -> Self {
        match error {
            SolveError::PatternNotFound(_) => Self::ScreenDots,
            SolveError::TooFewDepthPoints { .. } | SolveError::NoPlane => Self::ScreenDepth,
            SolveError::DegeneratePose | SolveError::Mirrored | SolveError::DotsInconsistent => {
                Self::ScreenGeometry
            }
            SolveError::KnownSizeInvalid => Self::SizeInvalid,
            SolveError::KnownSizeAspectMismatch { .. } => Self::SizeShape,
        }
    }

    fn of_session(error: &SessionError) -> Self {
        match error {
            SessionError::Camera(_) => Self::CameraLost,
            SessionError::Solve(e) => Self::of_solve(e),
            SessionError::Markers(
                MarkerError::TooFewBlobs { .. } | MarkerError::LayoutMismatch { .. },
            ) => Self::Markers,
            SessionError::EdgesCropped(sides) => Self::EdgesCropped(*sides),
            SessionError::ScreenNotMeasured | SessionError::MarkersNotMeasured => Self::Internal,
        }
    }
}

/// How long a condition has held without a break. The camera thread feeds it
/// every frame, so a dropped UI update cannot hide a flicker.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stability {
    since: Option<Instant>,
}

impl Stability {
    /// Records one observation at `now` and returns how long `holds` has been
    /// true in a row.
    pub fn observe(&mut self, holds: bool, now: Instant) -> Duration {
        if !holds {
            self.since = None;
            return Duration::ZERO;
        }
        now.saturating_duration_since(*self.since.get_or_insert(now))
    }

    pub fn reset(&mut self) {
        self.since = None;
    }
}

/// Handle to the camera thread. Dropping it stops the camera and the thread.
pub struct Worker {
    commands: Sender<Command>,
    in_flight: Arc<AtomicBool>,
}

impl Worker {
    /// Starts an idle thread: no camera is touched until a command asks.
    pub fn spawn(backend: Backend, events: UnboundedSender<Event>) -> Self {
        let (commands, receiver) = mpsc::channel();
        let in_flight = Arc::new(AtomicBool::new(false));
        let engine = Engine {
            backend,
            events,
            in_flight: in_flight.clone(),
            live: None,
            mode: Mode::Hold,
            session: 0,
            timeouts: 0,
            size_origin: None,
            require_edges: false,
            attempt: None,
            request_text: String::new(),
        };
        // The thread is detached: it ends when `commands` drops, and closing an
        // SDK pipeline can take a moment the UI thread must not wait for.
        std::thread::Builder::new()
            .name("calibration-camera".into())
            .spawn(move || engine.run(receiver))
            .expect("the OS refused to start a thread");
        Self {
            commands,
            in_flight,
        }
    }

    pub fn send(&self, command: Command) {
        // A send fails only once the thread has stopped, after which there is
        // no camera left to command.
        let _ = self.commands.send(command);
    }

    /// Tells the thread the UI has taken the last frame and wants the next.
    pub fn frame_consumed(&self) {
        self.in_flight.store(false, Ordering::Release);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Stop the stream now; the thread then sees the channel close and exits.
        let _ = self.commands.send(Command::Close);
    }
}

struct Live {
    session: Session,
    ready: Stability,
}

struct Engine {
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
    fn run(mut self, receiver: Receiver<Command>) {
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
                self.request_text = describe_request(&request);
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
                if let Some(live) = &mut self.live {
                    if let Err(e) = live.session.set_ir_exposure_us(us) {
                        self.fail(FailureKind::CameraLost, e.to_string(), false);
                    }
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
                let summary = attempt_summary(attempt);
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
                let (image, width, height) =
                    encode_rgb(&declared.review.color, u32::MAX, REVIEW_JPEG_QUALITY);
                // Kept for the user to save after the camera closes just below.
                self.remember_attempt(format!("finished\n{}", result_text(&declared)));
                let finished = Finished {
                    image,
                    width,
                    height,
                    size_origin: self.size_origin,
                    primary: outcome(declared),
                    measured: measured.map(outcome),
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
                        self.emit(SessionEvent::Preview(Box::new(preview_frame(
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
                    let all_matched =
                        preview.hint == MarkerHint::Ready && preview.matched.is_some();
                    let stable_for = live.ready.observe(all_matched, now);
                    if self.claim_frame() {
                        self.emit(SessionEvent::Markers(Box::new(marker_frame(
                            preview, stable_for,
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

/// The settings an attempt was opened with, as words for the bundle.
fn describe_request(request: &OpenRequest) -> String {
    use std::fmt::Write;
    let camera = &request.camera;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "camera: {} (serial {}, id {}, link {})",
        camera.name, camera.serial, camera.id, camera.link
    );
    let _ = writeln!(s, "capture settings: {:?}", request.settings);
    let _ = writeln!(s, "screen aspect: {:.4}", request.aspect);
    let _ = writeln!(s, "options: {:?}", request.options);
    let _ = writeln!(s, "declared size from: {:?}", request.size_origin);
    s
}

/// What a finished calibration measured, as words for the bundle.
fn result_text(c: &Calibration) -> String {
    use std::fmt::Write;
    let r = &c.review;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "screen {:.3} x {:.3} m ({:.1} in diagonal), size from {:?}",
        r.size_m[0], r.size_m[1], r.diagonal_in, r.size_source
    );
    let _ = writeln!(
        s,
        "plane rms {:.2} mm, max {:.2} mm, {} inliers",
        r.plane_rmse_m * 1000.0,
        r.plane_max_error_m * 1000.0,
        r.plane_inliers
    );
    let _ = writeln!(s, "dots used {} of {}", r.dots_used, r.dots_expected);
    let _ = writeln!(s, "edges: {:?}", r.edges);
    let _ = writeln!(s, "size check: {:?}", r.size_check);
    let _ = writeln!(s, "gravity aligned: {}", r.gravity_aligned);
    let _ = writeln!(
        s,
        "markers' largest offset from the layout: {:.4} m",
        r.marker_max_offset_m
    );
    for (i, p) in c.calibration.object_points.iter().enumerate() {
        let _ = writeln!(s, "marker {}: ({:.4}, {:.4}) m", i + 1, p.x, p.y);
    }
    s
}

/// The text at the top of a bundle: what ran, with what, and how it ended.
fn attempt_summary(attempt: &Attempt) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Odyssey Desktop {} on {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(s, "\n== request ==\n{}", attempt.request);
    let _ = writeln!(s, "== outcome ==\n{}", attempt.outcome);
    if let Some(frames) = &attempt.frames {
        let g = &frames.geometry;
        let _ = writeln!(s, "== camera geometry ==");
        let _ = writeln!(s, "color: {:?}", g.color);
        let _ = writeln!(s, "depth: {:?}", g.depth);
        let _ = writeln!(s, "infrared: {:?}", g.ir);
        let _ = writeln!(s, "color to depth: {:?}", g.color_to_depth);
        let _ = writeln!(s, "infrared to depth: {:?}", g.ir_to_depth);
        let _ = writeln!(s, "\n== frames ==");
        let _ = writeln!(s, "screen burst: {} frames", frames.screen_burst.len());
        if let Some(first) = frames.screen_burst.first() {
            let _ = writeln!(
                s,
                "  color {}x{}, depth {}x{} (16-bit, {} m per unit), infrared {}x{}",
                first.color.width(),
                first.color.height(),
                first.depth.width(),
                first.depth.height(),
                first.depth_unit_m,
                first.ir.width(),
                first.ir.height()
            );
        }
        let _ = writeln!(
            s,
            "marker burst: {} infrared frames; preview frame: {}",
            frames.marker_burst_ir.len(),
            frames.marker_preview_ir.is_some()
        );
    }
    s
}

/// Downscales `image` to at most `max_width` and encodes it as a JPEG data
/// URL. Returns the URL with the *original* dimensions, the coordinate system
/// of the overlays.
fn encode_rgb(image: &RgbImage, max_width: u32, quality: u8) -> (String, u32, u32) {
    let (w, h) = image.dimensions();
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, quality);
    let result = if w > max_width && max_width > 0 {
        let scaled_h = (u64::from(h) * u64::from(max_width) / u64::from(w)).max(1) as u32;
        let small = image::imageops::resize(image, max_width, scaled_h, FilterType::Triangle);
        encoder.encode(
            small.as_raw(),
            small.width(),
            small.height(),
            ExtendedColorType::Rgb8,
        )
    } else {
        encoder.encode(image.as_raw(), w, h, ExtendedColorType::Rgb8)
    };
    (data_url(result, bytes), w, h)
}

/// Encodes an infrared frame. It goes out as a three-channel JPEG: the embedded
/// web view did not paint single-channel JPEGs.
fn encode_gray(image: &GrayImage, quality: u8) -> (String, u32, u32) {
    let rgb = image::DynamicImage::ImageLuma8(image.clone()).into_rgb8();
    encode_rgb(&rgb, u32::MAX, quality)
}

fn data_url(result: image::ImageResult<()>, bytes: Vec<u8>) -> String {
    if let Err(e) = result {
        tracing::warn!("could not encode a preview frame: {e}");
        return String::new();
    }
    let mut url = String::from("data:image/jpeg;base64,");
    base64::engine::general_purpose::STANDARD.encode_string(bytes, &mut url);
    url
}

fn outcome(done: odyssey_autocalib::session::Calibration) -> Outcome {
    let r = &done.review;
    Outcome {
        screen_quad: r.screen_quad_px.map(|p| [p.x, p.y]),
        markers: r.marker_px.map(|p| [p.x, p.y]),
        size_m: r.size_m,
        diagonal_in: r.diagonal_in,
        size_source: r.size_source,
        size_check: r.size_check,
        measures: Measures::from(r),
        gravity_aligned: r.gravity_aligned,
        edges: r.edges,
        calibration: done.calibration,
    }
}

fn xy(p: nalgebra::Point2<f32>) -> [f32; 2] {
    [p.x, p.y]
}

fn preview_frame(preview: Preview, ready_for: Duration, require_edges: bool) -> PreviewFrame {
    let (image, width, height) =
        encode_rgb(&preview.color, PREVIEW_MAX_WIDTH, PREVIEW_JPEG_QUALITY);
    PreviewFrame {
        image,
        width,
        height,
        dots: preview.dots.iter().copied().map(xy).collect(),
        quad: preview.screen_quad.map(|q| q.map(xy)),
        dots_matched: preview.dots_matched,
        dots_expected: preview.dots_expected,
        depth_coverage: preview.depth_coverage,
        distance_m: preview.distance_m,
        tilt_deg: preview.tilt.map(|t| (t.pitch_deg, t.roll_deg)),
        cropped: match preview.edges {
            EdgeCheck::Cropped { sides } if require_edges => Some(sides),
            _ => None,
        },
        hint: preview.hint,
        ready_for,
    }
}

fn marker_frame(preview: MarkerPreview, stable_for: Duration) -> MarkerFrame {
    let (image, width, height) = encode_gray(&preview.ir, PREVIEW_JPEG_QUALITY);
    MarkerFrame {
        image,
        width,
        height,
        blobs: preview.blobs.iter().copied().map(xy).collect(),
        matched: preview.matched,
        hint: preview.hint,
        stable_for,
    }
}

#[cfg(test)]
mod tests {
    use odyssey_autocalib::pattern::Pattern;
    use odyssey_autocalib::session::KnownSize;

    use super::*;

    #[test]
    fn stability_counts_an_unbroken_run_and_resets_on_a_gap() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut s = Stability::default();
        assert_eq!(s.observe(true, t0), ms(0));
        assert_eq!(s.observe(true, t0 + ms(400)), ms(400));
        assert_eq!(s.observe(true, t0 + ms(1000)), ms(1000));
        assert_eq!(s.observe(false, t0 + ms(1100)), ms(0));
        // The run starts over after the gap rather than resuming.
        assert_eq!(s.observe(true, t0 + ms(1200)), ms(0));
        assert_eq!(s.observe(true, t0 + ms(1500)), ms(300));
    }

    #[test]
    fn demo_scene_follows_the_monitor_shape() {
        for aspect in [16.0 / 9.0, 21.0 / 9.0, 4.0 / 3.0, 9.0 / 16.0] {
            let scene = demo_scene(aspect, 40.0, true, 0.0);
            assert!((scene.aspect() - aspect).abs() < 1e-9);
            let [w, h] = scene.screen_size_m;
            assert!((w.hypot(h) / 0.0254 - 40.0).abs() < 1e-6);
            // What the camera is shown is what the wizard draws.
            assert_eq!(scene.pattern(), Pattern::new(aspect));
        }
    }

    #[test]
    fn demo_scene_survives_a_bad_aspect() {
        assert!((demo_scene(f64::NAN, 55.0, true, 0.0).aspect() - 16.0 / 9.0).abs() < 1e-9);
        assert!((demo_scene(0.0, 55.0, true, 0.0).aspect() - 16.0 / 9.0).abs() < 1e-9);
    }

    #[test]
    fn solve_errors_map_to_advice_categories() {
        use odyssey_autocalib::pattern::DetectFailure;
        let kind = |e: SolveError| FailureKind::of_session(&SessionError::Solve(e));
        assert_eq!(
            kind(SolveError::PatternNotFound(DetectFailure::NoDots {
                blobs: 0
            })),
            FailureKind::ScreenDots
        );
        assert_eq!(kind(SolveError::NoPlane), FailureKind::ScreenDepth);
        assert_eq!(
            kind(SolveError::TooFewDepthPoints { found: 3 }),
            FailureKind::ScreenDepth
        );
        assert_eq!(kind(SolveError::Mirrored), FailureKind::ScreenGeometry);
        assert_eq!(
            FailureKind::of_session(&SessionError::Camera(CameraError::Timeout)),
            FailureKind::CameraLost
        );
        assert_eq!(kind(SolveError::KnownSizeInvalid), FailureKind::SizeInvalid);
        assert_eq!(
            kind(SolveError::KnownSizeAspectMismatch {
                known_aspect: 1.6,
                screen_aspect: 1.78
            }),
            FailureKind::SizeShape
        );
        let sides = SideSet {
            top: true,
            left: true,
            ..SideSet::default()
        };
        assert_eq!(
            FailureKind::of_session(&SessionError::EdgesCropped(sides)),
            FailureKind::EdgesCropped(sides)
        );
    }

    /// How a scripted session on the camera thread ended.
    enum Ended {
        Finished(Box<Finished>),
        Failed(Failure),
    }

    /// Runs the whole flow on the camera thread against the simulated camera,
    /// measuring as soon as the preview is ready (or, with `measure_anyway`,
    /// at the first preview), and checks the camera is released at the end.
    fn run_session(
        backend: Backend,
        options: SessionOptions,
        origin: Option<SizeOrigin>,
        measure_anyway: bool,
    ) -> Ended {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let worker = Worker::spawn(backend, tx);

        worker.send(Command::ListCameras);
        let camera = match rx.blocking_recv() {
            Some(Event::Cameras(Ok(mut cameras))) => cameras.remove(0),
            other => panic!("expected a camera list, got {other:?}"),
        };

        worker.send(Command::Open {
            session: 7,
            request: OpenRequest {
                camera,
                settings: CaptureSettings::default(),
                aspect: 16.0 / 9.0,
                options,
                pattern_visible: true,
                size_origin: origin,
            },
        });
        let mut measuring = false;
        let mut outcome = None;
        let mut closed = false;
        let mut saved = None;
        let bundle_path = std::env::temp_dir().join(format!(
            "odyssey-worker-test-{}-{}.zip",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let deadline = Instant::now() + Duration::from_secs(120);
        while let Some(event) = rx.blocking_recv() {
            assert!(Instant::now() < deadline, "timed out");
            let Event::Session { session, event } = event else {
                continue;
            };
            assert_eq!(session, 7);
            match event {
                SessionEvent::Opened | SessionEvent::Progress(_) => {}
                SessionEvent::Preview(frame) => {
                    worker.frame_consumed();
                    assert!(frame.image.starts_with("data:image/jpeg;base64,"));
                    let ready =
                        frame.hint == Hint::Ready && frame.ready_for >= Duration::from_millis(300);
                    if (ready || measure_anyway) && !measuring {
                        measuring = true;
                        worker.send(Command::MeasureScreen);
                    }
                }
                SessionEvent::ScreenMeasured => worker.send(Command::SetMode(Mode::Markers)),
                SessionEvent::Markers(frame) => {
                    worker.frame_consumed();
                    if frame.stable_for >= Duration::from_millis(300) {
                        worker.send(Command::MeasureMarkers);
                    }
                }
                SessionEvent::MarkersMeasured => worker.send(Command::Finish),
                SessionEvent::Finished(f) => {
                    outcome = Some(Ended::Finished(f));
                    worker.send(Command::SaveAttempt {
                        path: bundle_path.clone(),
                    });
                }
                SessionEvent::Failed(f) => {
                    outcome = Some(Ended::Failed(f));
                    worker.send(Command::SaveAttempt {
                        path: bundle_path.clone(),
                    });
                    worker.send(Command::Close);
                }
                SessionEvent::Closed => closed = true,
                SessionEvent::AttemptSaved(result) => saved = Some(result),
            }
            // The bundle is written after the camera closes, from what the thread kept.
            if closed && saved.is_some() {
                break;
            }
        }
        assert!(closed, "the camera must be released at the end");
        let saved = saved
            .expect("a save result")
            .expect("the bundle is written");
        assert_eq!(saved.path, bundle_path);
        assert_eq!(saved.bytes, std::fs::metadata(&bundle_path).unwrap().len());
        let zip = zip::ZipArchive::new(std::fs::File::open(&bundle_path).unwrap()).unwrap();
        let names: Vec<_> = zip.file_names().map(str::to_owned).collect();
        assert!(names.iter().any(|n| n == "summary.txt"), "{names:?}");
        assert!(names.iter().any(|n| n == "calibration.log"), "{names:?}");
        if matches!(outcome, Some(Ended::Finished(_))) {
            // A finished attempt keeps every frame of its screen burst and the marker frames.
            let color = names.iter().filter(|n| n.ends_with("-color.png")).count();
            assert_eq!(color, SessionOptions::default().burst_frames, "{names:?}");
            assert!(names.iter().any(|n| n.starts_with("markers/")), "{names:?}");
        }
        std::fs::remove_file(&bundle_path).unwrap();
        drop(worker);
        outcome.expect("a result")
    }

    fn demo() -> Backend {
        Backend::Demo {
            overscan_fraction: 0.0,
        }
    }

    fn flat(known: Option<KnownSize>) -> SessionOptions {
        SessionOptions {
            known_size: known,
            require_visible_edges: true,
            ..SessionOptions::default()
        }
    }

    fn panel() -> Option<KnownSize> {
        let [width_m, height_m] = demo_scene(16.0 / 9.0, DEMO_DIAGONAL_IN, true, 0.0).screen_size_m;
        Some(KnownSize::Dimensions { width_m, height_m })
    }

    #[test]
    fn a_session_without_a_declared_size_measures_and_releases_the_camera() {
        let Ended::Finished(f) = run_session(demo(), SessionOptions::default(), None, false) else {
            panic!("failed")
        };
        assert!(f.image.starts_with("data:image/jpeg"));
        assert!(
            (f.primary.diagonal_in - DEMO_DIAGONAL_IN).abs() < 0.5,
            "{}",
            f.primary.diagonal_in
        );
        assert_eq!(f.primary.size_source, SizeSource::Measured);
        assert!(f.measured.is_none() && f.primary.size_check.is_none());
    }

    #[test]
    fn the_panel_size_anchors_a_flat_screen_and_the_camera_agrees() {
        let Ended::Finished(f) = run_session(demo(), flat(panel()), Some(SizeOrigin::Panel), false)
        else {
            panic!("failed")
        };
        assert_eq!(f.primary.size_source, SizeSource::KnownDimensions);
        let check = f.primary.size_check.expect("a size check");
        assert!(!check.disagrees(), "{check:?}");
        assert!(
            f.primary
                .measures
                .size_disagreement
                .is_some_and(|d| d < 0.02)
        );
        // Declared and measured results are both on offer, and the latter is the camera's.
        let measured = f.measured.as_ref().expect("the camera's own result");
        assert_eq!(measured.size_source, SizeSource::Measured);
        assert!(
            std::ptr::eq(f.outcome(false), &f.primary) && std::ptr::eq(f.outcome(true), measured)
        );
    }

    #[test]
    fn a_wrong_typed_size_disagrees_and_the_measured_result_corrects_it() {
        let wrong = Some(KnownSize::Diagonal { inches: 40.0 });
        let Ended::Finished(f) = run_session(demo(), flat(wrong), Some(SizeOrigin::Entered), false)
        else {
            panic!("failed")
        };
        assert!(f.primary.size_check.expect("check").disagrees());
        assert!((f.primary.diagonal_in - 40.0).abs() < 0.01);
        let measured = f.measured.as_ref().expect("measured");
        assert!(
            (measured.diagonal_in - DEMO_DIAGONAL_IN).abs() < 0.8,
            "{}",
            measured.diagonal_in
        );
    }

    #[test]
    fn an_overscanning_tv_cannot_be_measured_until_its_picture_is_fixed() {
        let backend = Backend::Demo {
            overscan_fraction: 0.03,
        };
        let Ended::Failed(f) = run_session(backend, flat(panel()), Some(SizeOrigin::Panel), true)
        else {
            panic!("finished")
        };
        assert!(
            matches!(f.kind, FailureKind::EdgesCropped(sides) if sides.count() == 4),
            "{f:?}"
        );
        assert_eq!(f.size_origin, Some(SizeOrigin::Panel));
    }

    #[test]
    fn a_projector_ignores_overscan() {
        let backend = Backend::Demo {
            overscan_fraction: 0.03,
        };
        let options = SessionOptions {
            require_visible_edges: false,
            ..SessionOptions::default()
        };
        let Ended::Finished(f) = run_session(backend, options, None, false) else {
            panic!("failed")
        };
        assert_eq!(f.primary.size_source, SizeSource::Measured);
    }

    #[test]
    fn a_camera_sized_flat_screen_is_calibrated_through_overscan() {
        use crate::cli::DEMO_OVERSCAN_MAX_PERCENT;
        use crate::views::calibrate::automatic::kind::{self, DisplayKind};
        use crate::views::calibrate::automatic::quality::{Verdict, assess};

        // What the wizard hands the session for a flat screen the camera sizes.
        let plan = kind::plan(DisplayKind::Flat, None, None, 16.0 / 9.0, true, None);
        assert!(plan.known_size.is_none() && !plan.require_visible_edges);

        for overscan in [0.03, DEMO_OVERSCAN_MAX_PERCENT / 100.0] {
            let options = SessionOptions {
                known_size: plan.known_size,
                require_visible_edges: plan.require_visible_edges,
                ..SessionOptions::default()
            };
            let backend = Backend::Demo {
                overscan_fraction: overscan,
            };
            let Ended::Finished(f) = run_session(backend, options, plan.origin, false) else {
                panic!("failed at {overscan} overscan")
            };
            let truth = demo_scene(16.0 / 9.0, DEMO_DIAGONAL_IN, true, overscan).truth();
            let at = format!("at {overscan} overscan");

            assert!(matches!(f.primary.edges, EdgeCheck::Cropped { .. }), "{at}");
            // The size is the whole picture, hidden edge included, not the panel.
            for (got, want) in f.primary.size_m.iter().zip(truth.size_m) {
                assert!((got / want - 1.0).abs() < 0.01, "{got} vs {want} {at}");
            }
            assert!(f.primary.diagonal_in > truth.diagonal_in() * 0.99, "{at}");
            // The markers land where the simulated ones are, in the picture's frame.
            for (got, want) in f
                .primary
                .calibration
                .object_points
                .iter()
                .zip(truth.markers_m)
            {
                let miss = (got.cast::<f64>() - want).norm();
                assert!(miss < 0.02, "marker {miss} m off {at}");
            }
            assert_ne!(assess(&f.primary.measures).verdict, Verdict::Poor, "{at}");
        }
    }

    #[test]
    fn a_tape_measured_flat_screen_is_calibrated_through_overscan_and_the_camera_agrees() {
        use crate::cli::DEMO_OVERSCAN_MAX_PERCENT;
        use crate::views::calibrate::automatic::kind::{self, DisplayKind};
        use crate::views::calibrate::automatic::quality::{Verdict, assess};
        use crate::views::calibrate::automatic::reference::{Reference, tape_line_px};

        let overscan = DEMO_OVERSCAN_MAX_PERCENT / 100.0;
        let truth = demo_scene(16.0 / 9.0, DEMO_DIAGONAL_IN, true, overscan).truth();

        // A tech lays a tape along the line a 1920 x 1080 screen draws and reads what
        // the displayed picture, overscan and all, really measures along it.
        let (width_px, height_px) = (1920, 1080);
        let line_px = f64::from(tape_line_px(width_px));
        let mm_per_px = truth.size_m[0] * 1000.0 / f64::from(width_px);
        let tape = Reference::from_tape(line_px, line_px * mm_per_px).expect("a plausible reading");

        // What the wizard hands the session for it.
        let plan = kind::plan(
            DisplayKind::Flat,
            None,
            None,
            16.0 / 9.0,
            false,
            Some(tape.known_size(width_px, height_px)),
        );
        assert_eq!(plan.origin, Some(SizeOrigin::Reference));
        assert!(!plan.require_visible_edges);

        let options = SessionOptions {
            known_size: plan.known_size,
            require_visible_edges: plan.require_visible_edges,
            ..SessionOptions::default()
        };
        let backend = Backend::Demo {
            overscan_fraction: overscan,
        };
        let Ended::Finished(f) = run_session(backend, options, plan.origin, false) else {
            panic!("failed at {overscan} overscan")
        };
        assert_eq!(f.primary.size_source, SizeSource::KnownDimensions);
        assert!(matches!(f.primary.edges, EdgeCheck::Cropped { .. }));
        // The result is the hand measurement, which is the whole displayed picture ...
        for (got, want) in f.primary.size_m.iter().zip(truth.size_m) {
            assert!((got / want - 1.0).abs() < 1e-6, "{got} vs {want}");
        }
        // ... and the camera, measuring the same picture, agrees with it.
        let check = f.primary.size_check.expect("a size check");
        assert!(!check.disagrees(), "{check:?}");
        assert_ne!(assess(&f.primary.measures).verdict, Verdict::Poor);
    }
}
