//! Creating the wizard, its camera thread and its event pumps.

use std::rc::Rc;

use gpui::{Context, Task, Window};
use odyssey_autocalib::camera::CaptureSettings;
use odyssey_autocalib::display::{self, DisplayLookup, LookupNote, LookupSource};
use odyssey_autocalib::session::SessionOptions;
use odyssey_hub_common::config::{self, MonitorKey, UiState};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use super::{CameraList, Stage, Wizard};
use crate::cli::CalibrateArgs;
use crate::hub::HubHandle;
use crate::runtime;
use crate::ui_state::UiStateWriter;
use crate::views::calibrate::automatic::kind::Finding;
use crate::views::calibrate::automatic::pattern_window::PatternEvent;
use crate::views::calibrate::automatic::screens::{SCREEN_COUNT, saved_screens};
use crate::views::calibrate::automatic::settings::SizeField;
use crate::views::calibrate::automatic::worker::{
    Backend, DEMO_DIAGONAL_IN, Event, Worker, demo_scene,
};
use crate::workspace::WorkspaceHandle;

impl Wizard {
    /// Creates the wizard's state, camera thread and event pumps. The
    /// command-line answers, if any, are consumed by this wizard.
    pub(crate) fn new(
        args: Option<CalibrateArgs>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let had_args = args
            .as_ref()
            .is_some_and(|args| *args != CalibrateArgs::default());
        let args = args.unwrap_or_default();
        let demo = args.demo;

        let monitors = cx.global::<WorkspaceHandle>().0.read(cx).monitors.clone();
        let (camera_events, camera_receiver) = unbounded_channel::<Event>();
        let (pattern_loss, pattern_receiver) = unbounded_channel::<PatternEvent>();
        let backend = if demo {
            Backend::Demo {
                overscan_fraction: args.demo_overscan_percent.unwrap_or(0.0) / 100.0,
            }
        } else {
            Backend::Hardware
        };
        let worker = Rc::new(Worker::spawn(backend, camera_events));

        let selected = monitors.read(cx).selected_key();
        let finding_task = selected
            .clone()
            .map(|key| Self::finding_task(key, demo, cx));

        cx.observe(&monitors, |this, _, cx| this.sync_selection(cx))
            .detach();
        // Auto-start waits for the hub to connect, which follows server start.
        let hub = cx.global::<HubHandle>().0.clone();
        cx.observe(&hub, |this, _, cx| this.maybe_auto_start(cx))
            .detach();
        cx.observe_window_bounds(window, |this, window, cx| {
            this.host_bounds = window.bounds();
            this.host_display = window.display(cx).map(|display| display.id());
        })
        .detach();
        // Closing the window drops the wizard, and with it the camera thread;
        // the pattern window is a separate window that has to be closed too.
        cx.on_release(|this, cx| {
            if let Some(pattern) = this.pattern.take() {
                pattern.close(cx);
            }
        })
        .detach();

        let tasks = vec![
            Self::pump_camera(camera_receiver, cx),
            Self::pump_pattern(pattern_receiver, cx),
            Self::load_startup(args.monitor, cx),
        ];

        Self {
            demo,
            monitors,
            stage: Stage::Setup,
            step: 0,
            worker,
            host: window.window_handle(),
            host_bounds: window.bounds(),
            host_display: window.display(cx).map(|display| display.id()),
            selected,
            screen_choice: args.screen_id,
            saved: [false; SCREEN_COUNT],
            ui_state: UiState::default(),
            ui_state_writer: cx.global::<UiStateWriter>().clone(),
            cameras: CameraList::Searching,
            camera_choice: args.camera.clone(),
            size_text: args.diagonal_in.map(|d| d.to_string()).unwrap_or_default(),
            size_field: SizeField::Diagonal,
            kind_choice: args.kind,
            finding: None,
            manual_size: args.diagonal_in.is_some(),
            reference: None,
            helper: None,
            helper_problem: None,
            use_measured: false,
            notes: Vec::new(),
            list_pending: false,
            setup_ready: false,
            auto_start: had_args && args.monitor.is_some(),
            color_exposure: args
                .rgb_exposure_us
                .unwrap_or(CaptureSettings::default().color_exposure_us),
            ir_exposure: SessionOptions::default().ir_exposure_us,
            session: 0,
            restarting: false,
            preview: None,
            markers: None,
            progress: None,
            finished: None,
            failure: None,
            saved_to: None,
            confirm_replace: false,
            saving: false,
            save_error: None,
            attempt_status: None,
            attempt_saving: false,
            pattern: None,
            pattern_loss,
            same_monitor: false,
            hidden: false,
            flow: None,
            camera_poll: Some(Self::poll_cameras(cx)),
            finding_task,
            save_task: None,
            _tasks: tasks,
        }
    }

    fn pump_camera(mut receiver: UnboundedReceiver<Event>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.recv().await {
                if this
                    .update(cx, |this, cx| this.on_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    fn pump_pattern(
        mut receiver: UnboundedReceiver<PatternEvent>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.recv().await {
                if this
                    .update(cx, |this, cx| this.on_pattern_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    /// Applies the monitor from the command line, then loads what Setup needs to
    /// default its choices.
    fn load_startup(monitor_arg: Option<usize>, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            if let Some(number) = monitor_arg {
                let applied = this.update(cx, |this, cx| {
                    let count = this.monitors.read(cx).monitors().len();
                    if number == 0 || number > count {
                        this.note(
                            format!("Display {number} isn't connected, so pick the screen below."),
                            cx,
                        );
                        this.auto_start = false;
                    } else {
                        this.monitors
                            .update(cx, |monitors, cx| monitors.select(number - 1, cx));
                    }
                });
                if applied.is_err() {
                    return;
                }
            }
            let state = runtime::spawn(async { config::ui_state_async().await.into_value() });
            let saved = runtime::spawn(read_saved_screens());
            let (Ok(state), Ok(saved)) = (state.await, saved.await) else {
                return;
            };
            this.update(cx, |this, cx| {
                this.ui_state = state;
                this.saved = saved.unwrap_or([false; SCREEN_COUNT]);
                this.setup_ready = true;
                cx.notify();
                this.maybe_auto_start(cx);
            })
            .ok();
        })
    }

    /// Reads the selected display's own size. The lookup reads files, so it
    /// stays off the UI thread.
    fn finding_task(key: MonitorKey, demo: bool, cx: &mut Context<Self>) -> Task<()> {
        let read = runtime::spawn(read_finding(key.clone(), demo));
        cx.spawn(async move |this, cx| {
            let Ok(finding) = read.await else { return };
            this.update(cx, |this, cx| {
                // The selection may have moved on while the files were read.
                if this.selected.as_ref() == Some(&key) {
                    this.finding = Some(finding);
                    cx.notify();
                    this.maybe_auto_start(cx);
                }
            })
            .ok();
        })
    }

    /// Follows the monitor choice: a new monitor needs its own size read and
    /// closes a helper whose picture is on the old one. A list refresh that
    /// leaves the choice alone changes nothing.
    pub(super) fn sync_selection(&mut self, cx: &mut Context<Self>) {
        let key = self.monitors.read(cx).selected_key();
        cx.notify();
        if key == self.selected {
            return;
        }
        self.selected = key.clone();
        let moved = self
            .helper
            .as_ref()
            .is_some_and(|helper| Some(&helper.monitor) != key.as_ref());
        if moved {
            self.cancel_helper(cx);
        }
        self.finding = None;
        self.finding_task = key.map(|key| Self::finding_task(key, self.demo, cx));
    }
}

pub(super) async fn read_saved_screens() -> Option<[bool; SCREEN_COUNT]> {
    tokio::task::spawn_blocking(|| {
        config::screens_dir()
            .map(|dir| saved_screens(&dir))
            .unwrap_or([false; SCREEN_COUNT])
    })
    .await
    .ok()
}

async fn read_finding(key: MonitorKey, demo: bool) -> Finding {
    if demo {
        let [width_m, height_m] = demo_scene(
            f64::from(key.width) / f64::from(key.height.max(1)),
            DEMO_DIAGONAL_IN,
            true,
            0.0,
        )
        .screen_size_m;
        return Finding::demo(width_m, height_m);
    }
    let lookup = tokio::task::spawn_blocking(move || {
        display::lookup(key.name.as_deref(), (key.width, key.height))
    })
    .await;
    match lookup {
        Ok(lookup) => Finding::from_lookup(&lookup),
        // A panicked lookup is no reason to block calibration.
        Err(_) => Finding::from_lookup(&DisplayLookup {
            edid: None,
            source: LookupSource::Unavailable,
            note: Some(LookupNote::NotFound),
        }),
    }
}
