//! The hook that creates the wizard and starts its event pumps.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::desktop::tao::event::{Event as TaoEvent, WindowEvent};
use dioxus::desktop::use_wry_event_handler;
use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_autocalib::camera::CaptureSettings;
use odyssey_autocalib::session::SessionOptions;
use odyssey_hub_common::config::{self, UiState};
use tokio::sync::mpsc::unbounded_channel;

use super::{CameraList, Stage, Wizard};
use crate::ServerReady;
use crate::cli::{self, CalibrateArgs};
use crate::components::use_monitor_selection;
use crate::views::calibrate::automatic::kind::Finding;
use crate::views::calibrate::automatic::pattern_window::{PatternEvent, PatternLoss};
use crate::views::calibrate::automatic::screens::SCREEN_COUNT;
use crate::views::calibrate::automatic::settings::SizeField;
use crate::views::calibrate::automatic::worker::{
    Backend, DEMO_DIAGONAL_IN, Event, Worker, demo_scene,
};

/// Creates the wizard's state, camera thread and event pump. `args` are the
/// command-line answers, consumed by the first wizard to open.
pub(crate) fn use_wizard() -> Wizard {
    let args = use_hook(|| cli::take_pending_calibrate().unwrap_or_default());
    let had_args = use_hook(|| args.clone() != CalibrateArgs::default());
    let demo = args.demo;

    let monitors = use_monitor_selection();
    let channel = use_hook(|| {
        let (tx, rx) = unbounded_channel::<Event>();
        (tx, Rc::new(RefCell::new(Some(rx))))
    });
    let losses = use_hook(|| {
        let (tx, rx) = unbounded_channel::<PatternEvent>();
        (tx, Rc::new(RefCell::new(Some(rx))))
    });
    let backend = if demo {
        Backend::Demo {
            overscan_fraction: args.demo_overscan_percent.unwrap_or(0.0) / 100.0,
        }
    } else {
        Backend::Hardware
    };
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
        size_text: use_signal(|| args.diagonal_in.map(|d| d.to_string()).unwrap_or_default()),
        size_field: use_signal(|| SizeField::Diagonal),
        kind_choice: use_signal(|| args.kind),
        finding: use_signal(|| None),
        manual_size: use_signal(|| args.diagonal_in.is_some()),
        reference: use_signal(|| None),
        helper: use_signal(|| None),
        helper_problem: use_signal(|| None),
        use_measured: use_signal(|| false),
        notes: use_signal(Vec::new),
        list_pending: use_signal(|| false),
        auto_start: use_signal(|| had_args && args.monitor.is_some()),
        color_exposure: use_signal(|| {
            args.rgb_exposure_us
                .unwrap_or(CaptureSettings::default().color_exposure_us)
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
        attempt_status: use_signal(|| None),
        attempt_saving: use_signal(|| false),
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
                    wizard.note(format!(
                        "Display {number} isn't connected, so pick the screen below."
                    ));
                    *wizard.auto_start.write() = false;
                } else {
                    wizard.monitors.select(number - 1);
                }
            }
            *wizard.ui_state.write() = config::ui_state_async().await.into_value();
            wizard.refresh_saved_screens().await;
        }
    });

    // Read the selected display's own size whenever the selection changes.
    // The lookup reads files, so it stays off the UI thread.
    use_resource(move || async move {
        let monitor = wizard.selected_monitor();
        // The helper's picture is on the monitor that was chosen when it opened;
        // a list refresh that leaves the choice alone must not close it.
        let moved = wizard
            .helper
            .peek()
            .as_ref()
            .is_some_and(|helper| Some(&helper.monitor) != monitor.as_ref());
        if moved {
            wizard.cancel_helper();
        }
        *wizard.finding.write() = None;
        let Some(key) = monitor else { return };
        let found = if demo {
            let [w, h] = demo_scene(
                f64::from(key.width) / f64::from(key.height.max(1)),
                DEMO_DIAGONAL_IN,
                true,
                0.0,
            )
            .screen_size_m;
            Finding::demo(w, h)
        } else {
            let lookup = tokio::task::spawn_blocking(move || {
                odyssey_autocalib::display::lookup(key.name.as_deref(), (key.width, key.height))
            })
            .await;
            match lookup {
                Ok(lookup) => Finding::from_lookup(&lookup),
                // A panicked lookup is no reason to block calibration.
                Err(_) => Finding::from_lookup(&odyssey_autocalib::display::DisplayLookup {
                    edid: None,
                    source: odyssey_autocalib::display::LookupSource::Unavailable,
                    note: Some(odyssey_autocalib::display::LookupNote::NotFound),
                }),
            }
        };
        *wizard.finding.write() = Some(found);
        wizard.maybe_auto_start();
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
                while let Some(event) = receiver.recv().await {
                    let loss = match event {
                        PatternEvent::Nudge(steps) => {
                            wizard.nudge_card(steps);
                            continue;
                        }
                        PatternEvent::Lost(loss) => loss,
                    };
                    let helper_covers = wizard.helper.peek().as_ref().map(|h| h.covers);
                    match helper_covers {
                        // The picture hid this window, so closing it is how you come back.
                        Some(true) => {
                            wizard.close_pattern();
                            dioxus::desktop::window().set_focus();
                            if loss == PatternLoss::WrongMonitor {
                                *wizard.helper_problem.write() = Some(
                                    "The picture didn't appear on the screen you chose. Your window manager moved it; \
                                     allow the \"Calibration pattern\" window to open fullscreen on the chosen display and try again."
                                        .into(),
                                );
                            }
                            continue;
                        }
                        Some(false) => {
                            wizard.cancel_helper();
                            wizard.note("The measuring screen was closed.".into());
                            continue;
                        }
                        None => {}
                    }
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
            if let TaoEvent::WindowEvent {
                window_id,
                event: WindowEvent::CloseRequested,
                ..
            } = event
                && *window_id == own_id
                && *wizard.stage.peek() != Stage::Setup
            {
                wizard.back_to_setup();
            }
        }
    });

    wizard
}
