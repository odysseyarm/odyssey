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

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
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
    Hint, KnownSize, MarkerHint, MarkerPreview, Preview, Session, SessionError, SessionOptions, Step,
};
use odyssey_autocalib::solve::SolveError;
use odyssey_autocalib::synthetic::{SyntheticBackend, SyntheticScene};
use odyssey_common::ScreenCalibration;
use tokio::sync::mpsc::UnboundedSender;

use super::quality::Measures;

/// Widest picture sent to the UI while previewing. A webview paints this at
/// roughly its natural size, and the JPEG stays small enough for ~15 frames a
/// second as a data URL.
const PREVIEW_MAX_WIDTH: u32 = 800;
const PREVIEW_JPEG_QUALITY: u8 = 72;
const REVIEW_JPEG_QUALITY: u8 = 85;
/// Consecutive missed frames tolerated before the camera counts as lost.
const MAX_CONSECUTIVE_TIMEOUTS: u32 = 3;

/// Diagonal of the simulated screen when the demo is not told one.
pub const DEMO_DEFAULT_DIAGONAL_IN: f64 = 55.0;

/// Which cameras the worker offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The simulated camera, and nothing else.
    Demo,
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
    match orbbec::runtime_status(None) {
        RuntimeStatus::Available { .. } => {
            OrbbecBackend::new().list().map_err(|e| ListError::Camera(e.to_string()))
        }
        unavailable => Err(ListError::Runtime(unavailable)),
    }
}

impl Backend {
    fn list(self) -> Result<Vec<CameraInfo>, ListError> {
        match self {
            Self::Demo => SyntheticBackend::default().list().map_err(|e| ListError::Camera(e.to_string())),
            Self::Hardware => list_hardware(),
        }
    }

    fn open(self, request: &OpenRequest) -> Result<Session, CameraError> {
        let stream = match self {
            Self::Demo => {
                let scene = demo_scene(
                    request.aspect,
                    demo_diagonal_in(request.options.known_size),
                    request.pattern_visible,
                );
                SyntheticBackend::new(scene).open(&request.camera, &request.settings)?
            }
            Self::Hardware => OrbbecBackend::new().open(&request.camera, &request.settings)?,
        };
        Ok(Session::start(stream, request.aspect, request.options.clone()))
    }
}

/// The diagonal the demo camera simulates: the declared one when there is one.
fn demo_diagonal_in(known: Option<KnownSize>) -> f64 {
    match known {
        Some(KnownSize::Diagonal { inches }) => inches,
        Some(KnownSize::Dimensions { width_m, height_m }) => width_m.hypot(height_m) / 0.0254,
        None => DEMO_DEFAULT_DIAGONAL_IN,
    }
}

/// The simulated scene for a screen of the given shape. The synthetic camera
/// renders the same dot grid the wizard draws because both derive it from this
/// aspect ratio, and it sits farther from a larger screen so the screen fills
/// a similar part of the picture.
pub fn demo_scene(aspect: f64, diagonal_in: f64, pattern_visible: bool) -> SyntheticScene {
    let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 16.0 / 9.0 };
    let diagonal_m = diagonal_in * 0.0254;
    let height = diagonal_m / (aspect * aspect + 1.0).sqrt();
    let mut scene = SyntheticScene {
        screen_size_m: [aspect * height, height],
        show_pattern: pattern_visible,
        // Deliver frames at the camera's rate instead of as fast as they render.
        realtime: true,
        ..SyntheticScene::default()
    };
    scene.screen_center_m.z = (2.0 * diagonal_in / DEMO_DEFAULT_DIAGONAL_IN).clamp(0.9, 3.5);
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
    Open { session: u64, request: OpenRequest },
    SetMode(Mode),
    SetIrExposure(u32),
    MeasureScreen,
    MeasureMarkers,
    /// Computes the result, then closes the camera.
    Finish,
    Close,
}

#[derive(Debug)]
pub enum Event {
    Cameras(Result<Vec<CameraInfo>, ListError>),
    /// Something about the camera session `session`; the UI drops events of
    /// sessions it has already abandoned.
    Session { session: u64, event: SessionEvent },
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

/// What the review screen needs once the calibration is done.
#[derive(Debug, Clone)]
pub struct Finished {
    pub calibration: ScreenCalibration<f32>,
    pub image: String,
    pub width: u32,
    pub height: u32,
    pub screen_quad: [[f32; 2]; 4],
    pub markers: [[f32; 2]; 6],
    pub size_m: [f64; 2],
    pub diagonal_in: f64,
    pub measures: Measures,
    pub gravity_aligned: bool,
}

/// What went wrong, coarsely enough to give advice; `details` keeps the
/// technical text for the Details section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub details: String,
    /// Whether the camera is still open and a retry can reuse it.
    pub camera_open: bool,
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
    Internal,
}

impl FailureKind {
    fn of_solve(error: &SolveError) -> Self {
        match error {
            SolveError::PatternNotFound(_) => Self::ScreenDots,
            SolveError::TooFewDepthPoints { .. } | SolveError::NoPlane => Self::ScreenDepth,
            SolveError::DegeneratePose
            | SolveError::Mirrored
            | SolveError::DotsInconsistent
            | SolveError::KnownSizeInvalid
            | SolveError::KnownSizeAspectMismatch { .. } => Self::ScreenGeometry,
        }
    }

    fn of_session(error: &SessionError) -> Self {
        match error {
            SessionError::Camera(_) => Self::CameraLost,
            SessionError::Solve(e) => Self::of_solve(e),
            SessionError::Markers(MarkerError::TooFewBlobs { .. } | MarkerError::LayoutMismatch { .. }) => Self::Markers,
            SessionError::EdgesCropped(_) => Self::ScreenGeometry,
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
        };
        // The thread is detached: it ends when `commands` drops, and closing an
        // SDK pipeline can take a moment the UI thread must not wait for.
        std::thread::Builder::new()
            .name("calibration-camera".into())
            .spawn(move || engine.run(receiver))
            .expect("the OS refused to start a thread");
        Self { commands, in_flight }
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
        let _ = self.events.send(Event::Session { session: self.session, event });
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
        self.mode = Mode::Hold;
        if !keep_camera {
            self.close_quietly();
        }
        self.emit(SessionEvent::Failed(Failure { kind, details, camera_open: keep_camera }));
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
                match self.backend.open(&request) {
                    Ok(opened) => {
                        tracing::info!("camera opened: {} (calibration session {session})", request.camera.name);
                        self.live = Some(Live { session: opened, ready: Stability::default() });
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
            Command::Close => self.close(),
        }
    }

    fn measure_screen(&mut self) {
        let Some(live) = &mut self.live else { return };
        self.mode = Mode::Hold;
        let (events, session) = (&self.events, self.session);
        let result = live.session.measure_screen(&mut |step| {
            let _ = events.send(Event::Session { session, event: SessionEvent::Progress(step) });
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
            let _ = events.send(Event::Session { session, event: SessionEvent::Progress(step) });
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

    fn finish(&mut self) {
        let Some(live) = &self.live else { return };
        match live.session.finish() {
            Ok(done) => {
                let r = &done.review;
                let (image, width, height) = encode_rgb(&r.color, u32::MAX, REVIEW_JPEG_QUALITY);
                let finished = Finished {
                    calibration: done.calibration,
                    image,
                    width,
                    height,
                    screen_quad: r.screen_quad_px.map(|p| [p.x, p.y]),
                    markers: r.marker_px.map(|p| [p.x, p.y]),
                    size_m: r.size_m,
                    diagonal_in: r.diagonal_in,
                    measures: Measures::from(r),
                    gravity_aligned: r.gravity_aligned,
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
                        self.emit(SessionEvent::Preview(Box::new(preview_frame(preview, ready_for))));
                    }
                }
                Err(e) => self.stream_error(e),
            },
            Mode::Markers => match live.session.preview_markers() {
                Ok(preview) => {
                    self.timeouts = 0;
                    let all_matched = preview.hint == MarkerHint::Ready && preview.matched.is_some();
                    let stable_for = live.ready.observe(all_matched, now);
                    if self.claim_frame() {
                        self.emit(SessionEvent::Markers(Box::new(marker_frame(preview, stable_for))));
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
        encoder.encode(small.as_raw(), small.width(), small.height(), ExtendedColorType::Rgb8)
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

fn xy(p: nalgebra::Point2<f32>) -> [f32; 2] {
    [p.x, p.y]
}

fn preview_frame(preview: Preview, ready_for: Duration) -> PreviewFrame {
    let (image, width, height) = encode_rgb(&preview.color, PREVIEW_MAX_WIDTH, PREVIEW_JPEG_QUALITY);
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
    use super::*;
    use odyssey_autocalib::pattern::Pattern;
    use odyssey_autocalib::session::SessionOptions;

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
            let scene = demo_scene(aspect, 40.0, true);
            assert!((scene.aspect() - aspect).abs() < 1e-9);
            let [w, h] = scene.screen_size_m;
            assert!((w.hypot(h) / 0.0254 - 40.0).abs() < 1e-6);
            // What the camera is shown is what the wizard draws.
            assert_eq!(scene.pattern(), Pattern::new(aspect));
        }
    }

    #[test]
    fn demo_scene_survives_a_bad_aspect() {
        assert!((demo_scene(f64::NAN, 55.0, true).aspect() - 16.0 / 9.0).abs() < 1e-9);
        assert!((demo_scene(0.0, 55.0, true).aspect() - 16.0 / 9.0).abs() < 1e-9);
    }

    #[test]
    fn solve_errors_map_to_advice_categories() {
        use odyssey_autocalib::pattern::DetectFailure;
        let kind = |e: SolveError| FailureKind::of_session(&SessionError::Solve(e));
        assert_eq!(kind(SolveError::PatternNotFound(DetectFailure::NoDots { blobs: 0 })), FailureKind::ScreenDots);
        assert_eq!(kind(SolveError::NoPlane), FailureKind::ScreenDepth);
        assert_eq!(kind(SolveError::TooFewDepthPoints { found: 3 }), FailureKind::ScreenDepth);
        assert_eq!(kind(SolveError::Mirrored), FailureKind::ScreenGeometry);
        assert_eq!(
            FailureKind::of_session(&SessionError::Camera(CameraError::Timeout)),
            FailureKind::CameraLost
        );
    }

    /// Runs the whole flow on the camera thread against the simulated camera
    /// and checks the thread opens and closes the stream when told to.
    #[test]
    fn worker_runs_a_full_session_and_releases_the_camera() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let worker = Worker::spawn(Backend::Demo, tx);

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
                options: SessionOptions::default(),
                pattern_visible: true,
            },
        });
        let mut saw_ready_preview = false;
        let mut measured = false;
        let mut finished = None;
        let mut closed = false;
        let deadline = Instant::now() + Duration::from_secs(120);
        while let Some(event) = rx.blocking_recv() {
            assert!(Instant::now() < deadline, "timed out");
            let Event::Session { session, event } = event else { continue };
            assert_eq!(session, 7);
            match event {
                SessionEvent::Opened => {}
                SessionEvent::Preview(frame) => {
                    worker.frame_consumed();
                    assert!(frame.image.starts_with("data:image/jpeg;base64,"));
                    if frame.hint == Hint::Ready && frame.ready_for >= Duration::from_millis(300) && !saw_ready_preview {
                        saw_ready_preview = true;
                        worker.send(Command::MeasureScreen);
                    }
                }
                SessionEvent::Progress(_) => {}
                SessionEvent::ScreenMeasured => {
                    measured = true;
                    worker.send(Command::SetMode(Mode::Markers));
                }
                SessionEvent::Markers(frame) => {
                    worker.frame_consumed();
                    if frame.stable_for >= Duration::from_millis(300) {
                        worker.send(Command::MeasureMarkers);
                    }
                }
                SessionEvent::MarkersMeasured => worker.send(Command::Finish),
                SessionEvent::Finished(f) => finished = Some(f),
                SessionEvent::Closed => {
                    closed = true;
                    break;
                }
                SessionEvent::Failed(f) => panic!("session failed: {f:?}"),
            }
        }
        assert!(saw_ready_preview && measured && closed);
        let finished = finished.expect("a finished calibration");
        assert!(finished.image.starts_with("data:image/jpeg"));
        assert!((finished.diagonal_in - DEMO_DEFAULT_DIAGONAL_IN).abs() < 0.5, "{}", finished.diagonal_in);
        drop(worker);
    }
}
