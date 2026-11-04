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

mod events;
mod helper;
mod markers;
mod navigation;
mod pattern;
mod saving;
mod session;
mod setup_answers;
mod stage;
mod use_wizard;

use std::rc::Rc;
use std::time::Duration;

use dioxus::core::Task;
use dioxus::prelude::*;
pub(crate) use helper::Helper;
use odyssey_autocalib::camera::CameraInfo;
use odyssey_autocalib::orbbec::RuntimeStatus;
use odyssey_autocalib::session::Step;
use odyssey_hub_common::config::{MonitorKey, UiState};
pub(crate) use stage::Stage;
use tokio::sync::mpsc::UnboundedSender;
pub(crate) use use_wizard::use_wizard;

use super::kind::{DisplayKind, Finding};
use super::pattern_window::{PatternDisplay, PatternEvent};
use super::reference::Reference;
use super::screens::SCREEN_COUNT;
use super::settings::SizeField;
use super::worker::{Command, Failure, Finished, MarkerFrame, PreviewFrame, SavedAttempt, Worker};
use crate::ServerReady;
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
    /// The camera library is not usable; Setup explains why and what to do.
    Unavailable(RuntimeStatus),
    Failed(String),
}

/// Where a saved calibration went.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct SavedInfo {
    pub screen_id: u8,
    pub path: String,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Wizard {
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
    ui_state_writer: UiStateWriter,
    pub cameras: Signal<CameraList>,
    pub camera_choice: Signal<Option<String>>,
    /// The size field's text. Blank means "use what the display says, or
    /// measure".
    pub size_text: Signal<String>,
    /// Which dimension the size field takes for a projector.
    pub size_field: Signal<SizeField>,
    /// The tech's explicit answer to "What are you calibrating?"; until then
    /// the remembered or default kind shows.
    kind_choice: Signal<Option<DisplayKind>>,
    /// What the display's own information says about its size; `None` while
    /// it is being read.
    pub finding: Signal<Option<Finding>>,
    /// Whether the "Not right? Enter the size yourself" field is open.
    pub manual_size: Signal<bool>,
    /// A size the tech measured by hand with the tape or card, and the
    /// monitor it describes.
    pub reference: Signal<Option<(MonitorKey, Reference)>>,
    /// The tape and card helper, while it is open.
    pub helper: Signal<Option<Helper>>,
    /// Why the helper would not open, shown beside its buttons: the page-top
    /// notes are out of sight by the time someone is down at the size section.
    pub helper_problem: Signal<Option<String>>,
    /// Show the camera's own measurement on the review instead of the declared
    /// size.
    pub use_measured: Signal<bool>,
    /// Command-line values that could not be honored, shown in Setup.
    pub notes: Signal<Vec<String>>,
    list_pending: Signal<bool>,
    /// Remembered setup answers and saved screen IDs have finished loading.
    setup_ready: Signal<bool>,
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
    /// The diagnostic bundle: whether one is being written, and how the last save went.
    pub attempt_status: Signal<Option<Result<SavedAttempt, String>>>,
    pub attempt_saving: Signal<bool>,

    // The pattern window and the wizard window stepping aside for it.
    pattern: Signal<Option<PatternDisplay>>,
    pattern_loss: Signal<UnboundedSender<PatternEvent>>,
    /// The pattern would cover the wizard, so the wizard hides while it shows.
    pub same_monitor: Signal<bool>,
    hidden: Signal<bool>,
    flow: Signal<Option<Task>>,
}

impl Wizard {
    fn send(self, command: Command) {
        self.worker.peek().send(command);
    }

    pub(crate) fn note(mut self, text: String) {
        self.notes.write().push(text);
    }

    pub(crate) fn set_stage(mut self, stage: Stage) {
        *self.stage.write() = stage;
        if stage != Stage::Setup {
            self.cancel_helper();
        }
        if stage != Stage::Problem {
            *self.step.write() = stage.step();
        }
    }
}
