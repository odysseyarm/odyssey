//! Camera sessions and the Position and Measuring stages.

use std::time::Duration;

use dioxus::prelude::*;
use odyssey_autocalib::camera::CaptureSettings;
use odyssey_autocalib::session::SessionOptions;

use super::{COUNTDOWN_SECONDS, READY_HOLD, SETTLE_TIMEOUT, Stage, Wizard};
use crate::components::screen_picker::monitor_key;
use crate::views::calibrate::automatic::pattern_window::{self, PatternMode};
use crate::views::calibrate::automatic::worker::{Command, Failure, FailureKind, OpenRequest};

impl Wizard {
    pub(super) fn aspect(self) -> Option<f64> {
        let key = self.selected_monitor()?;
        (key.height > 0).then(|| f64::from(key.width) / f64::from(key.height))
    }

    fn open_request(self, pattern_visible: bool) -> Option<OpenRequest> {
        let defaults = SessionOptions::default();
        let plan = self.size_plan()?;
        Some(OpenRequest {
            camera: self.camera()?,
            settings: CaptureSettings {
                color_exposure_us: *self.color_exposure.peek(),
                ..CaptureSettings::default()
            },
            aspect: self.aspect()?,
            options: SessionOptions {
                ir_exposure_us: *self.ir_exposure.peek(),
                known_size: plan.known_size,
                require_visible_edges: plan.require_visible_edges,
                ..defaults
            },
            pattern_visible,
            size_origin: plan.origin,
        })
    }

    /// Opens a fresh camera session. Events of earlier sessions are ignored
    /// from here on.
    fn start_session(mut self, pattern_visible: bool) {
        let Some(request) = self.open_request(pattern_visible) else {
            return;
        };
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
    pub(crate) fn restart_camera(self) {
        let pattern_visible = self.pattern.peek().is_some() || *self.hidden.peek();
        self.start_session(pattern_visible);
    }

    /// Enters Position: the pattern goes up on the target monitor and the
    /// camera starts. If that monitor is the one the wizard is on, the pattern
    /// would hide the wizard, so the pattern waits for the countdown instead.
    pub(crate) fn begin_position(mut self) {
        let Some(key) = self.selected_monitor() else {
            return;
        };
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
                size_origin: None,
            });
            return;
        }
        self.start_session(!covered);
    }

    /// Whether Next is available: the pattern has read "ready" long enough.
    pub(crate) fn position_ready(self) -> bool {
        self.preview
            .read()
            .as_ref()
            .is_some_and(|p| p.ready_for >= READY_HOLD)
    }

    pub(crate) fn measure_screen(mut self) {
        *self.progress.write() = None;
        self.set_stage(Stage::Measuring);
        self.send(Command::MeasureScreen);
    }

    /// Same-monitor mode: cover this window with the fullscreen picture, count
    /// down, show the pattern, wait for the camera to settle and measure, with
    /// the wizard behind the picture until the measurement is done. The window
    /// is not minimized: a minimized window's page stops running, and this flow
    /// runs in it.
    pub(crate) fn start_countdown(mut self) {
        self.cancel_flow();
        let task = spawn(async move {
            *self.hidden.write() = true;
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

    pub(super) fn cancel_flow(mut self) {
        if let Some(task) = self.flow.write().take() {
            task.cancel();
        }
    }

    /// Brings the wizard back in front after the pattern covered it.
    pub(super) fn restore_window(mut self) {
        if *self.hidden.peek() {
            dioxus::desktop::window().set_focus();
            *self.hidden.write() = false;
        }
    }
}
