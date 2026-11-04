//! The whole flow on the camera thread against the simulated camera.

use std::time::{Duration, Instant};

use odyssey_autocalib::camera::CaptureSettings;
use odyssey_autocalib::session::{EdgeCheck, Hint, KnownSize, SessionOptions, SizeSource};

use super::{
    Backend, Command, DEMO_DIAGONAL_IN, Event, Failure, FailureKind, Finished, Mode, OpenRequest,
    SessionEvent, Worker, demo_scene,
};
use crate::views::calibrate::automatic::kind::SizeOrigin;

/// How a scripted session on the camera thread ended.
enum Ended {
    Finished(Box<Finished>),
    Failed(Failure),
}

/// Runs the whole flow on the camera thread against the simulated camera,
/// measuring as soon as the preview is ready (or, with `measure_anyway`,
/// at the first preview), and checks the camera is released at the end.
fn run_session(
    backend: Backend,
    options: SessionOptions,
    origin: Option<SizeOrigin>,
    measure_anyway: bool,
) -> Ended {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let worker = Worker::spawn(backend, tx);

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
            options,
            pattern_visible: true,
            size_origin: origin,
        },
    });
    let mut measuring = false;
    let mut outcome = None;
    let mut closed = false;
    let mut saved = None;
    let bundle_path = std::env::temp_dir().join(format!(
        "odyssey-worker-test-{}-{}.zip",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let deadline = Instant::now() + Duration::from_secs(120);
    while let Some(event) = rx.blocking_recv() {
        assert!(Instant::now() < deadline, "timed out");
        let Event::Session { session, event } = event else {
            continue;
        };
        assert_eq!(session, 7);
        match event {
            SessionEvent::Opened | SessionEvent::Progress(_) => {}
            SessionEvent::Preview(frame) => {
                worker.frame_consumed();
                assert!(frame.image.starts_with("data:image/jpeg;base64,"));
                let ready =
                    frame.hint == Hint::Ready && frame.ready_for >= Duration::from_millis(300);
                if (ready || measure_anyway) && !measuring {
                    measuring = true;
                    worker.send(Command::MeasureScreen);
                }
            }
            SessionEvent::ScreenMeasured => worker.send(Command::SetMode(Mode::Markers)),
            SessionEvent::Markers(frame) => {
                worker.frame_consumed();
                if frame.matched.is_some() {
                    worker.send(Command::MeasureMarkers);
                }
            }
            SessionEvent::MarkersMeasured => worker.send(Command::Finish),
            SessionEvent::Finished(f) => {
                outcome = Some(Ended::Finished(f));
                worker.send(Command::SaveAttempt {
                    path: bundle_path.clone(),
                });
            }
            SessionEvent::Failed(f) => {
                outcome = Some(Ended::Failed(f));
                worker.send(Command::SaveAttempt {
                    path: bundle_path.clone(),
                });
                worker.send(Command::Close);
            }
            SessionEvent::Closed => closed = true,
            SessionEvent::AttemptSaved(result) => saved = Some(result),
        }
        // The bundle is written after the camera closes, from what the thread kept.
        if closed && saved.is_some() {
            break;
        }
    }
    assert!(closed, "the camera must be released at the end");
    let saved = saved
        .expect("a save result")
        .expect("the bundle is written");
    assert_eq!(saved.path, bundle_path);
    assert_eq!(saved.bytes, std::fs::metadata(&bundle_path).unwrap().len());
    let zip = zip::ZipArchive::new(std::fs::File::open(&bundle_path).unwrap()).unwrap();
    let names: Vec<_> = zip.file_names().map(str::to_owned).collect();
    assert!(names.iter().any(|n| n == "summary.txt"), "{names:?}");
    assert!(names.iter().any(|n| n == "calibration.log"), "{names:?}");
    if matches!(outcome, Some(Ended::Finished(_))) {
        // A finished attempt keeps every frame of its screen burst and the marker frames.
        let color = names.iter().filter(|n| n.ends_with("-color.png")).count();
        assert_eq!(color, SessionOptions::default().burst_frames, "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("markers/")), "{names:?}");
    }
    std::fs::remove_file(&bundle_path).unwrap();
    drop(worker);
    outcome.expect("a result")
}

fn demo() -> Backend {
    Backend::Demo {
        overscan_fraction: 0.0,
    }
}

fn flat(known: Option<KnownSize>) -> SessionOptions {
    SessionOptions {
        known_size: known,
        require_visible_edges: true,
        ..SessionOptions::default()
    }
}

fn panel() -> Option<KnownSize> {
    let [width_m, height_m] = demo_scene(16.0 / 9.0, DEMO_DIAGONAL_IN, true, 0.0).screen_size_m;
    Some(KnownSize::Dimensions { width_m, height_m })
}

#[test]
fn a_session_without_a_declared_size_measures_and_releases_the_camera() {
    let Ended::Finished(f) = run_session(demo(), SessionOptions::default(), None, false) else {
        panic!("failed")
    };
    assert!(f.image.starts_with("data:image/jpeg"));
    assert!(
        (f.primary.diagonal_in - DEMO_DIAGONAL_IN).abs() < 0.5,
        "{}",
        f.primary.diagonal_in
    );
    assert_eq!(f.primary.size_source, SizeSource::Measured);
    assert!(f.measured.is_none() && f.primary.size_check.is_none());
}

#[test]
fn the_panel_size_anchors_a_flat_screen_and_the_camera_agrees() {
    let Ended::Finished(f) = run_session(demo(), flat(panel()), Some(SizeOrigin::Panel), false)
    else {
        panic!("failed")
    };
    assert_eq!(f.primary.size_source, SizeSource::KnownDimensions);
    let check = f.primary.size_check.expect("a size check");
    assert!(!check.disagrees(), "{check:?}");
    assert!(
        f.primary
            .measures
            .size_disagreement
            .is_some_and(|d| d < 0.02)
    );
    // Declared and measured results are both on offer, and the latter is the camera's.
    let measured = f.measured.as_ref().expect("the camera's own result");
    assert_eq!(measured.size_source, SizeSource::Measured);
    assert!(
        std::ptr::eq(f.outcome(false), &raw const f.primary)
            && std::ptr::eq(f.outcome(true), measured)
    );
}

#[test]
fn a_wrong_typed_size_disagrees_and_the_measured_result_corrects_it() {
    let wrong = Some(KnownSize::Diagonal { inches: 40.0 });
    let Ended::Finished(f) = run_session(demo(), flat(wrong), Some(SizeOrigin::Entered), false)
    else {
        panic!("failed")
    };
    assert!(f.primary.size_check.expect("check").disagrees());
    assert!((f.primary.diagonal_in - 40.0).abs() < 0.01);
    let measured = f.measured.as_ref().expect("measured");
    assert!(
        (measured.diagonal_in - DEMO_DIAGONAL_IN).abs() < 0.8,
        "{}",
        measured.diagonal_in
    );
}

#[test]
fn an_overscanning_tv_cannot_be_measured_until_its_picture_is_fixed() {
    let backend = Backend::Demo {
        overscan_fraction: 0.03,
    };
    let Ended::Failed(f) = run_session(backend, flat(panel()), Some(SizeOrigin::Panel), true)
    else {
        panic!("finished")
    };
    assert!(
        matches!(f.kind, FailureKind::EdgesCropped(sides) if sides.count() == 4),
        "{f:?}"
    );
    assert_eq!(f.size_origin, Some(SizeOrigin::Panel));
}

#[test]
fn a_projector_ignores_overscan() {
    let backend = Backend::Demo {
        overscan_fraction: 0.03,
    };
    let options = SessionOptions {
        require_visible_edges: false,
        ..SessionOptions::default()
    };
    let Ended::Finished(f) = run_session(backend, options, None, false) else {
        panic!("failed")
    };
    assert_eq!(f.primary.size_source, SizeSource::Measured);
}

#[test]
fn a_camera_sized_flat_screen_is_calibrated_through_overscan() {
    use crate::cli::DEMO_OVERSCAN_MAX_PERCENT;
    use crate::views::calibrate::automatic::kind::{self, DisplayKind};
    use crate::views::calibrate::automatic::quality::{Verdict, assess};

    // What the wizard hands the session for a flat screen the camera sizes.
    let plan = kind::plan(DisplayKind::Flat, None, None, 16.0 / 9.0, true, None);
    assert!(plan.known_size.is_none() && !plan.require_visible_edges);

    for overscan in [0.03, DEMO_OVERSCAN_MAX_PERCENT / 100.0] {
        let options = SessionOptions {
            known_size: plan.known_size,
            require_visible_edges: plan.require_visible_edges,
            ..SessionOptions::default()
        };
        let backend = Backend::Demo {
            overscan_fraction: overscan,
        };
        let Ended::Finished(f) = run_session(backend, options, plan.origin, false) else {
            panic!("failed at {overscan} overscan")
        };
        let truth = demo_scene(16.0 / 9.0, DEMO_DIAGONAL_IN, true, overscan).truth();
        let at = format!("at {overscan} overscan");

        assert!(matches!(f.primary.edges, EdgeCheck::Cropped { .. }), "{at}");
        // The size is the whole picture, hidden edge included, not the panel.
        for (got, want) in f.primary.size_m.iter().zip(truth.size_m) {
            assert!((got / want - 1.0).abs() < 0.01, "{got} vs {want} {at}");
        }
        assert!(f.primary.diagonal_in > truth.diagonal_in() * 0.99, "{at}");
        // The markers land where the simulated ones are, in the picture's frame.
        for (got, want) in f
            .primary
            .calibration
            .object_points
            .iter()
            .zip(truth.markers_m)
        {
            let miss = (got.cast::<f64>() - want).norm();
            assert!(miss < 0.02, "marker {miss} m off {at}");
        }
        assert_ne!(assess(&f.primary.measures).verdict, Verdict::Poor, "{at}");
    }
}

#[test]
fn a_tape_measured_flat_screen_is_calibrated_through_overscan_and_the_camera_agrees() {
    use crate::cli::DEMO_OVERSCAN_MAX_PERCENT;
    use crate::views::calibrate::automatic::kind::{self, DisplayKind};
    use crate::views::calibrate::automatic::quality::{Verdict, assess};
    use crate::views::calibrate::automatic::reference::{Reference, tape_line_px};

    let overscan = DEMO_OVERSCAN_MAX_PERCENT / 100.0;
    let truth = demo_scene(16.0 / 9.0, DEMO_DIAGONAL_IN, true, overscan).truth();

    // A tech lays a tape along the line a 1920 x 1080 screen draws and reads what
    // the displayed picture, overscan and all, really measures along it.
    let (width_px, height_px) = (1920, 1080);
    let line_px = f64::from(tape_line_px(width_px));
    let mm_per_px = truth.size_m[0] * 1000.0 / f64::from(width_px);
    let tape = Reference::from_tape(line_px, line_px * mm_per_px).expect("a plausible reading");

    // What the wizard hands the session for it.
    let plan = kind::plan(
        DisplayKind::Flat,
        None,
        None,
        16.0 / 9.0,
        false,
        Some(tape.known_size(width_px, height_px)),
    );
    assert_eq!(plan.origin, Some(SizeOrigin::Reference));
    assert!(!plan.require_visible_edges);

    let options = SessionOptions {
        known_size: plan.known_size,
        require_visible_edges: plan.require_visible_edges,
        ..SessionOptions::default()
    };
    let backend = Backend::Demo {
        overscan_fraction: overscan,
    };
    let Ended::Finished(f) = run_session(backend, options, plan.origin, false) else {
        panic!("failed at {overscan} overscan")
    };
    assert_eq!(f.primary.size_source, SizeSource::KnownDimensions);
    assert!(matches!(f.primary.edges, EdgeCheck::Cropped { .. }));
    // The result is the hand measurement, which is the whole displayed picture ...
    for (got, want) in f.primary.size_m.iter().zip(truth.size_m) {
        assert!((got / want - 1.0).abs() < 1e-6, "{got} vs {want}");
    }
    // ... and the camera, measuring the same picture, agrees with it.
    let check = f.primary.size_check.expect("a size check");
    assert!(!check.disagrees(), "{check:?}");
    assert_ne!(assess(&f.primary.measures).verdict, Verdict::Poor);
}
