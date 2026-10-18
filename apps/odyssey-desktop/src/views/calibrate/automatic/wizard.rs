//! State and flow of the automatic calibration wizard.
//!
//! [`Wizard`] is a bundle of signal handles, so it is `Copy` and every step
//! view gets the whole controller. It owns the camera thread, the pattern
//! window and the stage machine; the views only draw it and call its methods.
//!
//! ```text
//! Setup ─▶ Position ─▶ Measuring ─▶ Markers ─▶ MeasuringMarkers ─▶ Review ─▶ Saved
//!    ▲         ▲            │            │             │              │
//!    └─────────┴──── Problem (any camera or measurement failure) ◀────┘
//! ```
//!
//! The camera streams only in Position, Measuring, Markers and
//! MeasuringMarkers. Every way out of those stages (back, cancel, failure,
//! the finished result, closing the window) closes the session.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use dioxus::desktop::tao::event::{Event as TaoEvent, WindowEvent};
use dioxus::desktop::use_wry_event_handler;
use dioxus_router::navigator;
use dioxus::core::Task;
use dioxus::{logger::tracing, prelude::*};
use odyssey_autocalib::camera::{CameraInfo, CaptureSettings};
use odyssey_autocalib::orbbec::RuntimeStatus;
use odyssey_autocalib::pattern::Pattern;
use odyssey_autocalib::session::{SessionOptions, Step};
use odyssey_hub_common::config::{self, MonitorKey, UiState};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use super::pattern_window::{self, PatternDisplay, PatternLoss, PatternMode};
use super::settings::{parse_diagonal, DiagonalError, COLOR_EXPOSURE_RANGE_US, IR_EXPOSURE_RANGE_US};
use super::screens::{default_screen, saved_screens, SCREEN_COUNT};
use super::worker::{
    Backend, Command, Event, Failure, FailureKind, Finished, ListError, MarkerFrame, Mode, OpenRequest,
    PreviewFrame, SessionEvent, Worker,
};
use crate::cli::{self, CalibrateArgs};
use crate::components::screen_picker::{monitor_key, MonitorSelection};
use crate::components::use_monitor_selection;
use crate::{Route, ServerReady};

/// How long the screen must read "ready" before Next lights up, so a hint that
/// flickers at the edge of acceptable never enables a measurement.
pub const READY_HOLD: Duration = Duration::from_millis(400);
/// How long all six markers must stay matched before measuring starts itself.
pub const MARKERS_AUTO_MEASURE: Duration = Duration::from_millis(1000);
/// Seconds counted down before the pattern appears when the wizard steps aside.
const COUNTDOWN_SECONDS: u32 = 5;
/// How long the countdown flow waits for the camera to settle on the pattern
/// before measuring anyway, so a bad view fails with advice instead of hanging.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(8);
const CAMERA_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Setup,
    Position,
    Measuring,
    Markers,
    MeasuringMarkers,
    Review,
    Saved,
    Problem,
}

impl Stage {
    pub const STEPS: [&'static str; 5] = ["Setup", "Position", "Measure", "Markers", "Review"];

    /// Index into [`Self::STEPS`]; `Saved` is one past the end so every step
    /// shows as done.
    fn step(self) -> usize {
        match self {
            Self::Setup => 0,
            Self::Position => 1,
            Self::Measuring => 2,
            Self::Markers | Self::MeasuringMarkers => 3,
            Self::Review => 4,
            Self::Saved => 5,
            Self::Problem => 0,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum CameraList {
    Searching,
    Found(Vec<CameraInfo>),
    /// The camera library is not usable; Setup explains why and what to do.
    Unavailable(RuntimeStatus),
    Failed(String),
}

/// Where a saved calibration went.
#[derive(Clone, PartialEq, Debug)]
pub struct SavedInfo {
    pub screen_id: u8,
    pub path: String,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Wizard {
    pub demo: bool,
    pub monitors: MonitorSelection,
    pub stage: Signal<Stage>,
    /// Stepper position; stays put while a problem is shown.
    pub step: Signal<usize>,
    worker: Signal<Rc<Worker>>,
    server_ready: Option<ServerReady>,

    // Setup answers.
    pub screen_choice: Signal<Option<u8>>,
    pub saved: Signal<[bool; SCREEN_COUNT]>,
    ui_state: Signal<UiState>,
    pub cameras: Signal<CameraList>,
    pub camera_choice: Signal<Option<String>>,
    pub diagonal_text: Signal<String>,
    /// Command-line values that could not be honored, shown in Setup.
    pub notes: Signal<Vec<String>>,
    list_pending: Signal<bool>,
    /// A command line that names the monitor skips Setup once a camera is
    /// known.
    auto_start: Signal<bool>,

    // Live camera state.
    pub color_exposure: Signal<u32>,
    pub ir_exposure: Signal<u32>,
    session: Signal<u64>,
    pub restarting: Signal<bool>,
    pub preview: Signal<Option<PreviewFrame>>,
    pub markers: Signal<Option<MarkerFrame>>,
    pub progress: Signal<Option<Step>>,

    // Outcome.
    pub finished: Signal<Option<Rc<Finished>>>,
    pub failure: Signal<Option<Failure>>,
    pub saved_to: Signal<Option<SavedInfo>>,
    pub confirm_replace: Signal<bool>,
    pub saving: Signal<bool>,
    pub save_error: Signal<Option<String>>,

    // The pattern window and the wizard window stepping aside for it.
    pattern: Signal<Option<PatternDisplay>>,
    pattern_loss: Signal<UnboundedSender<PatternLoss>>,
    /// The pattern would cover the wizard, so the wizard hides while it shows.
    pub same_monitor: Signal<bool>,
    hidden: Signal<bool>,
    flow: Signal<Option<Task>>,
}

/// Creates the wizard's state, camera thread and event pump. `args` are the
/// command-line answers, consumed by the first wizard to open.
pub fn use_wizard() -> Wizard {
    let args = use_hook(|| cli::take_pending_calibrate().unwrap_or_default());
    let had_args = use_hook(|| args.clone() != CalibrateArgs::default());
    let demo = args.demo;

    let monitors = use_monitor_selection();
    let channel = use_hook(|| {
        let (tx, rx) = unbounded_channel::<Event>();
        (tx, Rc::new(RefCell::new(Some(rx))))
    });
    let losses = use_hook(|| {
        let (tx, rx) = unbounded_channel::<PatternLoss>();
        (tx, Rc::new(RefCell::new(Some(rx))))
    });
    let backend = if demo { Backend::Demo } else { Backend::Hardware };
    let worker = use_signal({
        let tx = channel.0.clone();
        move || Rc::new(Worker::spawn(backend, tx))
    });
    let server_ready = use_hook(try_consume_context::<ServerReady>);

    let mut wizard = Wizard {
        demo,
        monitors,
        stage: use_signal(|| Stage::Setup),
        step: use_signal(|| 0),
        worker,
        server_ready,
        screen_choice: use_signal(|| args.screen_id),
        saved: use_signal(|| [false; SCREEN_COUNT]),
        ui_state: use_signal(UiState::default),
        cameras: use_signal(|| CameraList::Searching),
        camera_choice: use_signal(|| args.camera.clone()),
        diagonal_text: use_signal(|| args.diagonal_in.map(|d| d.to_string()).unwrap_or_default()),
        notes: use_signal(Vec::new),
        list_pending: use_signal(|| false),
        auto_start: use_signal(|| had_args && args.monitor.is_some()),
        color_exposure: use_signal(|| {
            args.rgb_exposure_us.unwrap_or(CaptureSettings::default().color_exposure_us)
        }),
        ir_exposure: use_signal(|| SessionOptions::default().ir_exposure_us),
        session: use_signal(|| 0),
        restarting: use_signal(|| false),
        preview: use_signal(|| None),
        markers: use_signal(|| None),
        progress: use_signal(|| None),
        finished: use_signal(|| None),
        failure: use_signal(|| None),
        saved_to: use_signal(|| None),
        confirm_replace: use_signal(|| false),
        saving: use_signal(|| false),
        save_error: use_signal(|| None),
        pattern: use_signal(|| None),
        pattern_loss: use_signal(|| losses.0.clone()),
        same_monitor: use_signal(|| false),
        hidden: use_signal(|| false),
        flow: use_signal(|| None),
    };

    // Apply the monitor from the command line, then load what Setup needs to
    // default its choices.
    use_future({
        let monitor_arg = args.monitor;
        move || async move {
            if let Some(number) = monitor_arg {
                if number > wizard.monitors.monitors().len() {
                    wizard.note(format!("Display {number} isn't connected, so pick the screen below."));
                    *wizard.auto_start.write() = false;
                } else {
                    wizard.monitors.select(number - 1);
                }
            }
            *wizard.ui_state.write() = config::ui_state_async().await.into_value();
            wizard.refresh_saved_screens().await;
        }
    });

    // Camera events arrive from the camera thread.
    use_future({
        let receiver = channel.1.clone();
        move || {
            let receiver = receiver.borrow_mut().take();
            async move {
                let Some(mut receiver) = receiver else { return };
                while let Some(event) = receiver.recv().await {
                    wizard.on_event(event);
                }
            }
        }
    });

    // Without the pattern window, or with it on the wrong monitor, there is
    // nothing valid for the camera to look at, so the calibration stops.
    use_future({
        let receiver = losses.1.clone();
        move || {
            let receiver = receiver.borrow_mut().take();
            async move {
                let Some(mut receiver) = receiver else { return };
                while let Some(loss) = receiver.recv().await {
                    if wizard.pattern.peek().is_none() {
                        continue;
                    }
                    tracing::info!("pattern window lost ({loss:?}); stopping the calibration");
                    wizard.back_to_setup();
                    wizard.note(
                        match loss {
                            PatternLoss::Closed => "The pattern screen was closed, so the calibration stopped.",
                            PatternLoss::WrongMonitor => {
                                "The pattern didn't appear on the screen you chose. Your window manager moved it; \
                                 allow the \"Calibration pattern\" window to open fullscreen on the chosen display and try again."
                            }
                        }
                        .into(),
                    );
                }
            }
        }
    });

    // Closing the main window only hides it to the tray; the camera must not
    // keep streaming behind a hidden window.
    use_wry_event_handler({
        let own_id = dioxus::desktop::window().id();
        move |event, _| {
            if let TaoEvent::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } = event {
                if *window_id == own_id && *wizard.stage.peek() != Stage::Setup {
                    wizard.back_to_setup();
                }
            }
        }
    });

    wizard
}

impl Wizard {
    fn send(self, command: Command) {
        self.worker.peek().send(command);
    }

    pub fn note(mut self, text: String) {
        self.notes.write().push(text);
    }

    pub fn set_stage(mut self, stage: Stage) {
        *self.stage.write() = stage;
        if stage != Stage::Problem {
            *self.step.write() = stage.step();
        }
    }

    // ------------------------------------------------------------- Setup --

    pub fn selected_monitor(self) -> Option<MonitorKey> {
        let monitors = self.monitors.monitors();
        monitors.get(self.monitors.selected()?).map(|m| m.key.clone())
    }

    /// The screen id Setup shows as chosen.
    pub fn screen_id(self) -> u8 {
        if let Some(chosen) = *self.screen_choice.read() {
            return chosen;
        }
        let remembered = self
            .selected_monitor()
            .and_then(|key| self.ui_state.read().calibration_screen(&key));
        default_screen(remembered, &self.saved.read())
    }

    /// The camera Setup shows as chosen: the picked one if it is still
    /// attached, otherwise the only one there is.
    pub fn camera(self) -> Option<CameraInfo> {
        let CameraList::Found(list) = &*self.cameras.read() else { return None };
        let chosen = self.camera_choice.read();
        let by_choice = chosen
            .as_deref()
            .and_then(|id| list.iter().find(|c| c.id == id || c.serial == id));
        match (by_choice, list.as_slice()) {
            (Some(camera), _) => Some(camera.clone()),
            (None, [only]) if chosen.is_none() => Some(only.clone()),
            _ => None,
        }
    }

    pub fn diagonal(self) -> Result<Option<f64>, DiagonalError> {
        parse_diagonal(&self.diagonal_text.read())
    }

    pub fn can_continue(self) -> bool {
        self.selected_monitor().is_some() && self.camera().is_some() && self.diagonal().is_ok()
    }

    pub async fn refresh_saved_screens(mut self) {
        let found = tokio::task::spawn_blocking(|| {
            config::screens_dir().map(|dir| saved_screens(&dir)).unwrap_or([false; SCREEN_COUNT])
        })
        .await
        .unwrap_or([false; SCREEN_COUNT]);
        *self.saved.write() = found;
    }

    /// Asks the camera thread for the attached cameras, unless it is still
    /// answering the last request.
    pub fn request_camera_list(mut self) {
        if !*self.list_pending.peek() {
            *self.list_pending.write() = true;
            self.send(Command::ListCameras);
        }
    }

    /// Looks for cameras while Setup is on screen. The returned future ends
    /// with the component that runs it, which is what stops the polling.
    pub async fn poll_cameras(self) {
        loop {
            self.request_camera_list();
            tokio::time::sleep(CAMERA_POLL_INTERVAL).await;
        }
    }

    // ---------------------------------------------------------- Position --

    fn aspect(self) -> Option<f64> {
        let key = self.selected_monitor()?;
        (key.height > 0).then(|| f64::from(key.width) / f64::from(key.height))
    }

    fn open_request(self, pattern_visible: bool) -> Option<OpenRequest> {
        let defaults = SessionOptions::default();
        Some(OpenRequest {
            camera: self.camera()?,
            settings: CaptureSettings {
                color_exposure_us: *self.color_exposure.peek(),
                ..CaptureSettings::default()
            },
            aspect: self.aspect()?,
            options: SessionOptions {
                ir_exposure_us: *self.ir_exposure.peek(),
                screen_diagonal_in: self.diagonal().ok().flatten(),
                ..defaults
            },
            pattern_visible,
        })
    }

    /// Opens a fresh camera session. Events of earlier sessions are ignored
    /// from here on.
    fn start_session(mut self, pattern_visible: bool) {
        let Some(request) = self.open_request(pattern_visible) else { return };
        let session = *self.session.peek() + 1;
        *self.session.write() = session;
        *self.preview.write() = None;
        *self.markers.write() = None;
        *self.progress.write() = None;
        *self.failure.write() = None;
        *self.restarting.write() = true;
        self.send(Command::Open { session, request });
    }

    /// Restarts the stream with the current exposure settings.
    pub fn restart_camera(self) {
        let pattern_visible = self.pattern.peek().is_some() || *self.hidden.peek();
        self.start_session(pattern_visible);
    }

    fn open_pattern(mut self, mode: PatternMode) -> bool {
        let (Some(handle), Some(aspect)) = (self.monitors.selected_handle(), self.aspect()) else {
            return false;
        };
        let display = pattern_window::open(
            &dioxus::desktop::window(),
            handle,
            Pattern::new(aspect),
            mode,
            self.pattern_loss.peek().clone(),
        );
        *self.pattern.write() = Some(display);
        true
    }

    fn close_pattern(mut self) {
        // Dropping the display closes its window.
        *self.pattern.write() = None;
    }

    fn show(self, mode: PatternMode) {
        if let Some(display) = &*self.pattern.peek() {
            display.show(mode);
        }
    }

    /// Enters Position: the pattern goes up on the target monitor and the
    /// camera starts. If that monitor is the one the wizard is on, the pattern
    /// would hide the wizard, so the pattern waits for the countdown instead.
    pub fn begin_position(mut self) {
        let Some(key) = self.selected_monitor() else { return };
        self.cancel_flow();
        self.close_pattern();
        self.notes.write().clear();
        let window = dioxus::desktop::window();
        let current = window.current_monitor().map(|m| monitor_key(&m));
        let covered = pattern_window::covers_window(&window, &key, current);
        *self.same_monitor.write() = covered;
        self.set_stage(Stage::Position);
        if !covered && !self.open_pattern(PatternMode::Pattern) {
            self.problem(Failure {
                kind: FailureKind::Internal,
                details: "the selected monitor is no longer available".into(),
                camera_open: false,
            });
            return;
        }
        self.start_session(!covered);
    }

    /// Whether Next is available: the pattern has read "ready" long enough.
    pub fn position_ready(self) -> bool {
        self.preview.read().as_ref().is_some_and(|p| p.ready_for >= READY_HOLD)
    }

    pub fn measure_screen(mut self) {
        *self.progress.write() = None;
        self.set_stage(Stage::Measuring);
        self.send(Command::MeasureScreen);
    }

    /// Same-monitor mode: step aside, count down, show the pattern, wait for
    /// the camera to settle and measure, with the wizard window hidden until
    /// the measurement is done.
    pub fn start_countdown(mut self) {
        self.cancel_flow();
        let task = spawn(async move {
            let host = dioxus::desktop::window();
            *self.hidden.write() = true;
            host.set_minimized(true);
            if !self.open_pattern(PatternMode::Countdown(COUNTDOWN_SECONDS)) {
                self.restore_window();
                return;
            }
            // The simulated camera draws the pattern only if it knows it is
            // up, so it restarts with the pattern shown.
            if self.demo {
                self.start_session(true);
            }
            for left in (1..=COUNTDOWN_SECONDS).rev() {
                self.show(PatternMode::Countdown(left));
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            self.show(PatternMode::Pattern);

            let started = tokio::time::Instant::now();
            while !self.position_ready() && started.elapsed() < SETTLE_TIMEOUT {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            self.measure_screen();
        });
        *self.flow.write() = Some(task);
    }

    fn cancel_flow(mut self) {
        if let Some(task) = self.flow.write().take() {
            task.cancel();
        }
    }

    fn restore_window(mut self) {
        if *self.hidden.peek() {
            let window = dioxus::desktop::window();
            window.set_minimized(false);
            window.set_focus();
            *self.hidden.write() = false;
        }
    }

    // ----------------------------------------------------------- Markers --

    pub fn measure_markers(mut self) {
        *self.progress.write() = None;
        self.set_stage(Stage::MeasuringMarkers);
        self.send(Command::MeasureMarkers);
    }

    pub fn set_ir_exposure(mut self, us: u32) {
        *self.ir_exposure.write() = us.clamp(*IR_EXPOSURE_RANGE_US.start(), *IR_EXPOSURE_RANGE_US.end());
    }

    /// Sends the chosen infrared exposure to the camera. Each change costs the
    /// camera a few frames to settle, so callers apply it once the slider rests.
    pub fn apply_ir_exposure(self) {
        self.send(Command::SetIrExposure(*self.ir_exposure.peek()));
    }

    pub fn set_color_exposure(mut self, us: u32) {
        *self.color_exposure.write() = us.clamp(*COLOR_EXPOSURE_RANGE_US.start(), *COLOR_EXPOSURE_RANGE_US.end());
    }

    fn enter_markers(mut self) {
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

    // ------------------------------------------------------- Leaving etc --

    /// Stops the camera and takes the pattern down.
    fn release_hardware(mut self) {
        self.cancel_flow();
        self.send(Command::Close);
        self.close_pattern();
        self.restore_window();
        *self.restarting.write() = false;
    }

    pub fn back_to_setup(mut self) {
        self.release_hardware();
        // Ignore anything the stopped session still reports.
        *self.session.write() += 1;
        *self.preview.write() = None;
        *self.markers.write() = None;
        *self.finished.write() = None;
        *self.saved_to.write() = None;
        *self.confirm_replace.write() = false;
        *self.save_error.write() = None;
        self.set_stage(Stage::Setup);
        spawn(async move { self.refresh_saved_screens().await });
    }

    /// Calibrate another screen: back to Setup with the screen number chosen afresh and
    /// the size cleared, since it described the last screen.
    pub fn calibrate_another(mut self) {
        *self.screen_choice.write() = None;
        self.diagonal_text.write().clear();
        self.back_to_setup();
    }

    /// Leaves the wizard for the Calibrate page.
    pub fn exit(self) {
        self.release_hardware();
        navigator().push(Route::Calibrate {});
    }

    /// Zero lives on Home.
    pub fn go_home(self) {
        navigator().push(Route::Home {});
    }

    fn problem(mut self, failure: Failure) {
        self.close_pattern();
        self.restore_window();
        *self.failure.write() = Some(failure);
        self.set_stage(Stage::Problem);
    }

    /// The button on the problem screen: back to the step that can try again.
    pub fn retry(self) {
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
            FailureKind::Internal => self.back_to_setup(),
            _ => self.begin_position(),
        }
    }

    // ------------------------------------------------------------ Review --

    /// Saves unless that screen already holds a calibration and the tech has not
    /// confirmed replacing it.
    pub fn save(mut self, replace_confirmed: bool) {
        let Some(finished) = self.finished.peek().clone() else { return };
        let screen_id = self.screen_id();
        if self.saved.peek()[usize::from(screen_id)] && !replace_confirmed {
            *self.confirm_replace.write() = true;
            return;
        }
        *self.confirm_replace.write() = false;
        *self.saving.write() = true;
        *self.save_error.write() = None;
        let key = self.selected_monitor();
        spawn(async move {
            let calibration = finished.calibration;
            let written = tokio::task::spawn_blocking(move || {
                let dir = config::screens_dir().map_err(|e| e.to_string())?;
                odyssey_autocalib::session::save_calibration(&dir, screen_id, &calibration)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            *self.saving.write() = false;
            match written {
                Ok(path) => {
                    if let Some(key) = key {
                        // Reload before saving so a field changed elsewhere
                        // meanwhile is not overwritten.
                        let mut state = config::ui_state_async().await.into_value();
                        state.remember_calibration_screen(key, screen_id);
                        if let Err(e) = config::ui_state_save_async(&state).await {
                            tracing::warn!("could not remember the screen: {e}");
                        }
                        *self.ui_state.write() = state;
                    }
                    self.refresh_saved_screens().await;
                    *self.saved_to.write() = Some(SavedInfo { screen_id, path: path.display().to_string() });
                    self.set_stage(Stage::Saved);
                }
                Err(e) => {
                    tracing::error!("could not save the calibration: {e}");
                    *self.save_error.write() = Some(e);
                }
            }
        });
    }

    // ------------------------------------------------------------ Events --

    fn on_event(mut self, event: Event) {
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
            SessionEvent::Markers(frame) => {
                let matched = frame.matched.is_some() && frame.stable_for >= MARKERS_AUTO_MEASURE;
                *self.markers.write() = Some(*frame);
                if matched && *self.stage.peek() == Stage::Markers {
                    self.measure_markers();
                }
            }
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
                *self.saved_to.write() = None;
                *self.confirm_replace.write() = false;
                *self.save_error.write() = None;
                self.close_pattern();
                self.restore_window();
                self.set_stage(Stage::Review);
            }
            SessionEvent::Failed(failure) => self.problem(failure),
            SessionEvent::Closed => {}
        }
    }

    /// Skips Setup when the command line already answered it: a monitor was
    /// named and a camera is known. Waits for the hub server to have started,
    /// because a second app instance exits right after launch and must not
    /// open the pattern window on its way out.
    fn maybe_auto_start(mut self) {
        if !*self.auto_start.peek() || *self.stage.peek() != Stage::Setup {
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
            self.note(format!("Camera '{name}' isn't connected, so pick one below."));
            *self.camera_choice.write() = None;
        }
    }
}
