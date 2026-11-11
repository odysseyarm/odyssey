//! State and flow of the automatic calibration wizard.
//!
//! [`Wizard`] is a GPUI model: step views hold an `Entity<Wizard>`, read its
//! plain fields and call its methods, which `cx.notify()` on every change. It
//! owns the camera thread, the pattern window and the stage machine; the views
//! only draw it.
//!
//! ```text
//! Setup ─▶ Position ─▶ Measuring ─▶ Markers ─▶ MeasuringMarkers ─▶ Review ─▶ Saved
//!    ▲         ▲            │            │             │              │
//!    └─────────┴──── Problem (any camera or measurement failure) ◀────┘
//! ```
//!
//! Position and Markers stream previews; the measuring stages capture bursts.
//! Leaving the flow closes the session. A measurement failure may retain an
//! idle camera for retry, but no countdown task may advance the failed flow.
//!
//! Setup must finish loading remembered answers before either manual or CLI
//! continuation. Saving snapshots one review result; returning to Setup
//! invalidates its completion so a late disk write cannot change the next flow.

mod events;
mod helper;
mod init;
mod markers;
mod navigation;
mod pattern;
mod saving;
mod session;
mod setup_answers;
mod stage;

use std::rc::Rc;
use std::time::Duration;

use gpui::{AnyWindowHandle, Bounds, DisplayId, Entity, Pixels, Task};
pub(crate) use helper::Helper;
use odyssey_autocalib::camera::CameraInfo;
use odyssey_autocalib::orbbec::RuntimeStatus;
use odyssey_autocalib::session::Step;
use odyssey_hub_common::config::{MonitorKey, UiState};
pub(crate) use stage::Stage;
use tokio::sync::mpsc::UnboundedSender;

use super::kind::{DisplayKind, Finding};
use super::pattern_window::{PatternDisplay, PatternEvent};
use super::reference::Reference;
use super::screens::SCREEN_COUNT;
use super::settings::SizeField;
use super::worker::{Failure, Finished, MarkerFrame, PreviewFrame, SavedAttempt, Worker};
use crate::components::screen_picker::MonitorSelection;
use crate::ui_state::UiStateWriter;

/// How long the screen must read "ready" before Next lights up, so a hint that
/// flickers at the edge of acceptable never enables a measurement.
pub(crate) const READY_HOLD: Duration = Duration::from_millis(400);
/// Seconds counted down before the pattern appears when it covers the wizard.
const COUNTDOWN_SECONDS: u32 = 5;
/// How long the countdown flow waits for the camera to settle on the pattern
/// before measuring anyway, so a bad view fails with advice instead of hanging.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(8);
const CAMERA_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum CameraList {
    Searching,
    Found(Vec<CameraInfo>),
    /// The camera library is not usable. Setup explains why and what to do.
    Unavailable(RuntimeStatus),
    Failed(String),
}

/// Where a saved calibration went.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct SavedInfo {
    pub screen_id: u8,
    pub path: String,
}

pub(crate) struct Wizard {
    pub demo: bool,
    pub monitors: Entity<MonitorSelection>,
    pub stage: Stage,
    /// Stepper position. Stays put while a problem is shown.
    pub step: usize,
    worker: Rc<Worker>,
    /// The wizard's own window, where the pattern may land on top of it.
    host: AnyWindowHandle,
    host_bounds: Bounds<Pixels>,
    host_display: Option<DisplayId>,
    /// The selected monitor, mirrored from `monitors` so getters need no `cx`.
    selected: Option<MonitorKey>,

    pub screen_choice: Option<u8>,
    pub saved: [bool; SCREEN_COUNT],
    ui_state: UiState,
    ui_state_writer: UiStateWriter,
    pub cameras: CameraList,
    pub camera_choice: Option<String>,
    /// The size field's text. Blank means "use what the display says, or
    /// measure".
    pub size_text: String,
    /// Which dimension the size field takes for a projector.
    pub size_field: SizeField,
    /// The tech's explicit answer to "What are you calibrating?" Until then
    /// the remembered or default kind shows. Set it with [`Wizard::set_kind`].
    kind_choice: Option<DisplayKind>,
    /// What the display's own information says about its size. `None` while
    /// it is being read.
    pub finding: Option<Finding>,
    /// Whether the "Not right? Enter the size yourself" field is open.
    pub manual_size: bool,
    /// A size the tech measured by hand with the tape or card, and the
    /// monitor it describes.
    pub reference: Option<(MonitorKey, Reference)>,
    /// The tape and card helper, while it is open.
    pub helper: Option<Helper>,
    /// Why the helper would not open, shown beside its buttons: the page-top
    /// notes are out of sight by the time someone is down at the size section.
    pub helper_problem: Option<String>,
    /// Show the camera's own measurement on the review instead of the declared
    /// size.
    pub use_measured: bool,
    /// Command-line values that could not be honored, shown in Setup.
    pub notes: Vec<String>,
    list_pending: bool,
    /// Remembered setup answers and saved screen IDs have finished loading.
    setup_ready: bool,
    /// A command line that names the monitor skips Setup once a camera is
    /// known.
    auto_start: bool,

    pub color_exposure: u32,
    pub ir_exposure: u32,
    session: u64,
    pub restarting: bool,
    pub preview: Option<PreviewFrame>,
    pub markers: Option<MarkerFrame>,
    pub progress: Option<Step>,

    pub finished: Option<Rc<Finished>>,
    pub failure: Option<Failure>,
    pub saved_to: Option<SavedInfo>,
    pub confirm_replace: bool,
    pub saving: bool,
    pub save_error: Option<String>,
    /// The diagnostic bundle: whether one is being written, and how the last save went.
    pub attempt_status: Option<Result<SavedAttempt, String>>,
    pub attempt_saving: bool,

    pattern: Option<PatternDisplay>,
    pattern_loss: UnboundedSender<PatternEvent>,
    /// The pattern would cover the wizard, so the wizard hides while it shows.
    pub same_monitor: bool,
    hidden: bool,

    /// The countdown flow. Dropping it cancels the flow.
    flow: Option<Task<()>>,
    camera_poll: Option<Task<()>>,
    finding_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    /// Event pumps and one-shot loaders that live as long as the wizard.
    _tasks: Vec<Task<()>>,
}

impl Wizard {
    pub(crate) fn is_hidden(&self) -> bool {
        self.hidden
    }
}
