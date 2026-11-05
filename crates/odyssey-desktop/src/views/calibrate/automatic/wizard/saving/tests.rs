use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dioxus::core::NoOpMutations;
use dioxus::prelude::*;
use nalgebra::{Matrix3, Point3, UnitQuaternion};
use odyssey_autocalib::session::{EdgeCheck, SizeSource, load_calibration, save_calibration};
use odyssey_common::ScreenCalibration;
use odyssey_hub_common::config::{self, MonitorKey, UiState};

use super::super::{CameraList, Stage, Wizard};
use crate::components::screen_picker::MonitorSelection;
use crate::ui_state::{UiStateWriter, use_ui_state_writer};
use crate::views::calibrate::automatic::quality::Measures;
use crate::views::calibrate::automatic::screens::SCREEN_COUNT;
use crate::views::calibrate::automatic::settings::SizeField;
use crate::views::calibrate::automatic::worker::{Backend, Finished, Outcome, Worker};

fn monitor() -> MonitorKey {
    MonitorKey {
        name: Some("Save regression".into()),
        x: 0,
        y: 0,
        width: 1280,
        height: 720,
    }
}

fn outcome(scale: f32) -> Outcome {
    Outcome {
        calibration: ScreenCalibration {
            surface: None,
            rotation: UnitQuaternion::identity(),
            homography: Matrix3::new(scale, 0.0, 0.0, 0.0, scale, 0.0, 0.0, 0.0, 1.0),
            object_points: [Point3::new(0.2, 0.3, 0.0); 6],
        },
        screen_quad: [[0.0; 2]; 4],
        markers: [[0.0; 2]; 6],
        size_m: [1.0, 1.0],
        diagonal_in: 55.0,
        size_source: SizeSource::Measured,
        size_check: None,
        measures: Measures {
            plane_rmse_m: 0.001,
            dots_used: 20,
            dots_expected: 20,
            size_disagreement: None,
        },
        gravity_aligned: false,
        edges: EdgeCheck::Visible,
        seed_circle: None,
    }
}

fn finished() -> Finished {
    Finished {
        image: String::new(),
        width: 1280,
        height: 720,
        size_origin: None,
        primary: outcome(1.0),
        measured: Some(outcome(2.0)),
    }
}

fn fixture(writer: UiStateWriter) -> Wizard {
    let (events, _) = tokio::sync::mpsc::unbounded_channel();
    let mut saved = [false; SCREEN_COUNT];
    saved[0] = true;
    Wizard {
        demo: true,
        monitors: MonitorSelection::for_test(monitor(), writer),
        stage: Signal::new(Stage::Review),
        step: Signal::new(Stage::Review.step()),
        worker: Signal::new(Rc::new(Worker::spawn(
            Backend::Demo {
                overscan_fraction: 0.0,
            },
            events,
        ))),
        server_ready: None,
        screen_choice: Signal::new(Some(0)),
        saved: Signal::new(saved),
        ui_state: Signal::new(UiState::default()),
        ui_state_writer: writer,
        cameras: Signal::new(CameraList::Searching),
        camera_choice: Signal::new(None),
        size_text: Signal::new(String::new()),
        size_field: Signal::new(SizeField::Diagonal),
        kind_choice: Signal::new(None),
        finding: Signal::new(None),
        manual_size: Signal::new(false),
        reference: Signal::new(None),
        helper: Signal::new(None),
        helper_problem: Signal::new(None),
        use_measured: Signal::new(true),
        notes: Signal::new(Vec::new()),
        list_pending: Signal::new(false),
        setup_ready: Signal::new(true),
        auto_start: Signal::new(false),
        color_exposure: Signal::new(0),
        ir_exposure: Signal::new(0),
        session: Signal::new(1),
        restarting: Signal::new(false),
        preview: Signal::new(None),
        markers: Signal::new(None),
        progress: Signal::new(None),
        finished: Signal::new(Some(Rc::new(finished()))),
        failure: Signal::new(None),
        saved_to: Signal::new(None),
        confirm_replace: Signal::new(false),
        saving: Signal::new(false),
        save_error: Signal::new(None),
        attempt_status: Signal::new(None),
        attempt_saving: Signal::new(false),
        pattern: Signal::new(None),
        pattern_loss: Signal::new(tokio::sync::mpsc::unbounded_channel().0),
        same_monitor: Signal::new(false),
        hidden: Signal::new(false),
        flow: Signal::new(None),
    }
}

#[component]
fn Confirmation(wizard: Wizard, removed: Rc<Cell<bool>>) -> Element {
    use_drop(move || removed.set(true));
    use_hook(move || wizard.save(true));
    rsx! {}
}

#[component]
fn Harness(capture: Rc<Cell<Option<Wizard>>>, removed: Rc<Cell<bool>>) -> Element {
    let writer = use_ui_state_writer();
    let wizard = use_hook(move || fixture(writer));
    capture.set(Some(wizard));
    let saving = *wizard.saving.read();
    let stage = *wizard.stage.read();
    rsx! {
        div { "{stage:?}: {saving}" }
        if *wizard.confirm_replace.read() {
            Confirmation { wizard, removed }
        }
    }
}

async fn finish_save(dom: &mut VirtualDom, wizard: Wizard) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            dom.render_immediate(&mut NoOpMutations);
            if !*wizard.saving.peek() {
                break;
            }
            dom.wait_for_work().await;
        }
    })
    .await
    .expect("save must complete after its confirmation component unmounts");
}

async fn save_scenarios() {
    let dir = config::screens_dir().expect("simulation provides a screens directory");
    let original = outcome(0.5).calibration;
    save_calibration(&dir, 0, &original).expect("initial calibration is valid");
    let capture = Rc::new(Cell::new(None));
    let removed = Rc::new(Cell::new(false));
    let mut dom = VirtualDom::new_with_props(
        Harness,
        HarnessProps {
            capture: Rc::clone(&capture),
            removed: Rc::clone(&removed),
        },
    );
    dom.rebuild(&mut NoOpMutations);
    let mut wizard = capture.get().expect("harness captures the mounted wizard");

    dom.in_scope(ScopeId::APP, || wizard.save(false));
    assert!(*wizard.confirm_replace.peek());
    assert!(!*wizard.saving.peek());
    assert_eq!(
        load_calibration(&dir, 0)
            .expect("original calibration remains readable")
            .homography,
        original.homography
    );

    let (release, blocked) = std::sync::mpsc::channel();
    let (started, ready) = tokio::sync::oneshot::channel();
    let blocker = tokio::task::spawn_blocking(move || {
        let _ = started.send(());
        let _ = blocked.recv();
    });
    ready.await.expect("blocking task signals before waiting");
    dom.render_immediate(&mut NoOpMutations);
    dom.render_immediate(&mut NoOpMutations);
    assert!(removed.get());
    assert!(*wizard.saving.peek());
    assert!(!*wizard.confirm_replace.peek());
    wizard.use_measured.set(false);
    release.send(()).expect("blocking task waits for release");
    blocker
        .await
        .expect("blocking task exits without panicking");
    finish_save(&mut dom, wizard).await;

    let measured = outcome(2.0).calibration;
    assert_eq!(*wizard.stage.peek(), Stage::Saved);
    assert!(wizard.save_error.peek().is_none());
    assert_eq!(
        load_calibration(&dir, 0)
            .expect("confirmed calibration is readable")
            .homography,
        measured.homography
    );
    assert_eq!(
        wizard
            .saved_to
            .peek()
            .as_ref()
            .expect("successful save records its destination")
            .screen_id,
        0
    );
    assert_eq!(
        config::ui_state_async()
            .await
            .into_value()
            .calibration_screen(&monitor()),
        Some(0),
    );

    let mut invalid = finished();
    invalid.primary.calibration.homography = Matrix3::zeros();
    invalid.measured = None;
    wizard.finished.set(Some(Rc::new(invalid)));
    wizard.stage.set(Stage::Review);
    dom.in_scope(ScopeId::APP, || wizard.save(false));
    finish_save(&mut dom, wizard).await;
    assert_eq!(*wizard.stage.peek(), Stage::Review);
    assert!(wizard.save_error.peek().is_some());
    assert!(!*wizard.confirm_replace.peek());
    assert_eq!(
        load_calibration(&dir, 0)
            .expect("failed save preserves readable calibration")
            .homography,
        measured.homography
    );

    wizard.finished.set(Some(Rc::new(finished())));
    dom.in_scope(ScopeId::APP, || wizard.save(false));
    assert!(*wizard.confirm_replace.peek());
    finish_save(&mut dom, wizard).await;
    assert_eq!(*wizard.stage.peek(), Stage::Saved);
    assert!(wizard.save_error.peek().is_none());
    assert_eq!(
        load_calibration(&dir, 0)
            .expect("retried calibration is readable")
            .homography,
        outcome(1.0).calibration.homography
    );
}

#[test]
fn confirmed_overwrite_survives_dialog_unmount_and_errors_can_retry() {
    const ROOT_ENV: &str = "ODYSSEY_CALIBRATION_SAVE_TEST_ROOT";
    if let Some(root) = std::env::var_os(ROOT_ENV) {
        config::set_simulation_config_root(root.into()).unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap()
            .block_on(save_scenarios());
        return;
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("odyssey-save-{}-{stamp}", std::process::id()));
    let test = concat!(
        module_path!(),
        "::confirmed_overwrite_survives_dialog_unmount_and_errors_can_retry"
    );
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test.split_once("::").unwrap().1, "--nocapture"])
        .env(ROOT_ENV, &root)
        .status()
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        status.success(),
        "isolated calibration save regression failed"
    );
}
