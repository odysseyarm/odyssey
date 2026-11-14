//! Camera sessions and the Position and Measuring stages.

use std::time::{Duration, Instant};

use gpui::Context;
use odyssey_autocalib::camera::CaptureSettings;
use odyssey_autocalib::session::SessionOptions;

use super::{COUNTDOWN_SECONDS, READY_HOLD, SETTLE_TIMEOUT, Stage, Wizard};
use crate::views::calibrate::automatic::pattern_window::PatternMode;
use crate::views::calibrate::automatic::worker::{Command, Failure, FailureKind, OpenRequest};

impl Wizard {
    pub(super) fn aspect(&self) -> Option<f64> {
        let key = self.selected.as_ref()?;
        (key.height > 0).then(|| f64::from(key.width) / f64::from(key.height))
    }

    fn open_request(&self, pattern_visible: bool) -> Option<OpenRequest> {
        let defaults = SessionOptions::default();
        let plan = self.size_plan()?;
        Some(OpenRequest {
            camera: self.camera()?,
            settings: CaptureSettings {
                color_exposure_us: self.color_exposure,
                ..CaptureSettings::default()
            },
            aspect: self.aspect()?,
            options: SessionOptions {
                ir_exposure_us: self.ir_exposure,
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
    fn start_session(&mut self, pattern_visible: bool, cx: &mut Context<Self>) {
        let Some(request) = self.open_request(pattern_visible) else {
            return;
        };
        self.session += 1;
        self.preview = None;
        self.markers = None;
        self.progress = None;
        self.failure = None;
        self.restarting = true;
        cx.notify();
        let session = self.session;
        self.send(Command::Open { session, request }, cx);
    }

    /// Restarts the stream with the current exposure settings.
    pub(crate) fn restart_camera(&mut self, cx: &mut Context<Self>) {
        let pattern_visible = self.pattern.is_some() || self.hidden;
        self.start_session(pattern_visible, cx);
    }

    /// Enters Position: the pattern goes up on the target monitor and the
    /// camera starts. If that monitor is the one the wizard is on, the pattern
    /// would hide the wizard, so the pattern waits for the countdown instead.
    pub(crate) fn begin_position(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_none() {
            return;
        }
        self.cancel_flow();
        self.close_pattern(cx);
        self.notes.clear();
        let covered = self.pattern_covers_host(cx);
        self.same_monitor = covered;
        self.set_stage(Stage::Position, cx);
        if !covered && !self.open_pattern(PatternMode::Pattern, cx) {
            self.problem(
                Failure {
                    kind: FailureKind::Internal,
                    details: "the selected monitor is no longer available".into(),
                    camera_open: false,
                    size_origin: None,
                },
                cx,
            );
            return;
        }
        self.start_session(!covered, cx);
    }

    /// Whether Next is available: the pattern has read "ready" long enough.
    pub(crate) fn position_ready(&self) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|p| p.ready_for >= READY_HOLD)
    }

    pub(crate) fn measure_screen(&mut self, cx: &mut Context<Self>) {
        self.progress = None;
        self.set_stage(Stage::Measuring, cx);
        self.send(Command::MeasureScreen, cx);
    }

    /// Same-monitor mode: cover this window with the fullscreen picture, count
    /// down, show the pattern, wait for the camera to settle and measure, with
    /// the wizard behind the picture until the measurement is done. The window
    /// is not minimized, so it stays mapped and keeps receiving events.
    pub(crate) fn start_countdown(&mut self, cx: &mut Context<Self>) {
        self.cancel_flow();
        self.flow = Some(cx.spawn(async move |this, cx| {
            let opened = this.update(cx, |this, cx| {
                this.hidden = true;
                cx.notify();
                if !this.open_pattern(PatternMode::Countdown(COUNTDOWN_SECONDS), cx) {
                    this.restore_window(cx);
                    return false;
                }
                // The simulated camera draws the pattern only if it knows it is
                // up, so it restarts with the pattern shown.
                if this.demo {
                    this.start_session(true, cx);
                }
                true
            });
            if !matches!(opened, Ok(true)) {
                return;
            }
            for left in (1..=COUNTDOWN_SECONDS).rev() {
                if this
                    .update(cx, |this, cx| this.show(PatternMode::Countdown(left), cx))
                    .is_err()
                {
                    return;
                }
                cx.background_executor().timer(Duration::from_secs(1)).await;
            }
            if this
                .update(cx, |this, cx| this.show(PatternMode::Pattern, cx))
                .is_err()
            {
                return;
            }

            let started = Instant::now();
            loop {
                match this.update(cx, |this, _| this.position_ready()) {
                    Ok(false) if started.elapsed() < SETTLE_TIMEOUT => {}
                    Ok(_) => break,
                    Err(_) => return,
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            let _ = this.update(cx, Self::measure_screen);
        }));
    }

    pub(super) fn cancel_flow(&mut self) {
        self.flow = None;
    }

    /// Brings the wizard back in front after the pattern covered it.
    pub(super) fn restore_window(&mut self, cx: &mut Context<Self>) {
        if self.hidden {
            self.focus_host(cx);
            self.hidden = false;
            cx.notify();
        }
    }
}
