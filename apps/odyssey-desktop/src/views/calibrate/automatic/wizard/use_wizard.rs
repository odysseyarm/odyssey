//! The hook that creates the wizard and starts its event pumps.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::desktop::tao::event::{Event as TaoEvent, WindowEvent};
use dioxus::desktop::use_wry_event_handler;
use dioxus::prelude::*;
use odyssey_autocalib::camera::CaptureSettings;
use odyssey_autocalib::display::{self, DisplayLookup, LookupNote, LookupSource};
use odyssey_autocalib::session::SessionOptions;
use odyssey_hub_common::config::{self, MonitorKey, UiState};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::{CameraList, Stage, Wizard};
use crate::ServerReady;
use crate::cli::{self, CalibrateArgs};
use crate::components::use_monitor_selection;
use crate::views::calibrate::automatic::kind::Finding;
use crate::views::calibrate::automatic::pattern_window::PatternEvent;
use crate::views::calibrate::automatic::screens::SCREEN_COUNT;
use crate::views::calibrate::automatic::settings::SizeField;
use crate::views::calibrate::automatic::worker::{
    Backend, DEMO_DIAGONAL_IN, Event, Worker, demo_scene,
};

/// An unbounded channel whose receiving half is moved into the one task that
/// pumps it. The component can rerun the pump's closure, but only the first
/// run gets the receiver.
struct Pump<T> {
    sender: UnboundedSender<T>,
    receiver: Rc<RefCell<Option<UnboundedReceiver<T>>>>,
}

impl<T> Clone for Pump<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            receiver: Rc::clone(&self.receiver),
        }
    }
}

fn use_pump<T: 'static>() -> Pump<T> {
    use_hook(|| {
        let (sender, receiver) = unbounded_channel::<T>();
        Pump {
            sender,
            receiver: Rc::new(RefCell::new(Some(receiver))),
        }
    })
}

/// Creates the wizard's state, camera thread and event pumps. The
/// command-line answers are consumed by the first wizard to open.
pub(crate) fn use_wizard() -> Wizard {
    let args = use_hook(|| cli::take_pending_calibrate().unwrap_or_default());
    let had_args = use_hook(|| args.clone() != CalibrateArgs::default());
    let demo = args.demo;

    let monitors = use_monitor_selection();
    let camera_events = use_pump::<Event>();
    let pattern_events = use_pump::<PatternEvent>();
    let backend = if demo {
        Backend::Demo {
            overscan_fraction: args.demo_overscan_percent.unwrap_or(0.0) / 100.0,
        }
    } else {
        Backend::Hardware
    };
    let worker = use_signal({
        let sender = camera_events.sender.clone();
        move || Rc::new(Worker::spawn(backend, sender))
    });
    let server_ready = use_hook(try_consume_context::<ServerReady>);

    let wizard = Wizard {
        demo,
        monitors,
        stage: use_signal(|| Stage::Setup),
        step: use_signal(|| 0),
        worker,
        server_ready,
        screen_choice: use_signal(|| args.screen_id),
        saved: use_signal(|| [false; SCREEN_COUNT]),
        ui_state: use_signal(UiState::default),
        ui_state_writer: use_context::<crate::ui_state::UiStateWriter>(),
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
        setup_ready: use_signal(|| false),
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
        pattern_loss: use_signal(|| pattern_events.sender.clone()),
        same_monitor: use_signal(|| false),
        hidden: use_signal(|| false),
        flow: use_signal(|| None),
    };

    use_monitor_argument(wizard, args.monitor);
    use_display_finding(wizard, demo);
    use_camera_events(wizard, camera_events);
    use_pattern_events(wizard, pattern_events);
    use_close_request(wizard);

    wizard
}

/// Applies the monitor from the command line, then loads what Setup needs to
/// default its choices.
fn use_monitor_argument(mut wizard: Wizard, monitor_arg: Option<usize>) {
    use_future(move || async move {
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
        *wizard.setup_ready.write() = true;
        wizard.maybe_auto_start();
    });
}

/// Reads the selected display's own size whenever the selection changes. The
/// lookup reads files, so it stays off the UI thread.
fn use_display_finding(mut wizard: Wizard, demo: bool) {
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
        *wizard.finding.write() = Some(read_finding(key, demo).await);
        wizard.maybe_auto_start();
    });
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

/// Camera events arrive from the camera thread.
fn use_camera_events(wizard: Wizard, pump: Pump<Event>) {
    use_future(move || {
        let receiver = pump.receiver.borrow_mut().take();
        async move {
            let Some(mut receiver) = receiver else { return };
            while let Some(event) = receiver.recv().await {
                wizard.on_event(event);
            }
        }
    });
}

/// Pattern window events arrive from the pattern window's own page.
fn use_pattern_events(wizard: Wizard, pump: Pump<PatternEvent>) {
    use_future(move || {
        let receiver = pump.receiver.borrow_mut().take();
        async move {
            let Some(mut receiver) = receiver else { return };
            while let Some(event) = receiver.recv().await {
                wizard.on_pattern_event(event);
            }
        }
    });
}

/// Closing the main window only hides it to the tray; the camera must not keep
/// streaming behind a hidden window.
fn use_close_request(wizard: Wizard) {
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
}
