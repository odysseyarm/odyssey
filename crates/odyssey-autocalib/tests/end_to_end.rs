//! Drives a whole calibration against the synthetic camera and compares the
//! result with the scene's ground truth.

use nalgebra::{Matrix3, Point2};
use odyssey_autocalib::camera::{CameraBackend, CaptureSettings};
use odyssey_autocalib::markers::MarkerError;
use odyssey_autocalib::session::{
    Calibration, EdgeCheck, Hint, KnownSize, MARKER_FRAMES, MarkerHint, Session, SessionError,
    SessionOptions, SizeSource, Step,
};
use odyssey_autocalib::solve::SolveError;
use odyssey_autocalib::synthetic::{SyntheticBackend, SyntheticScene, Truth};

fn open(scene: &SyntheticScene, settings: &CaptureSettings, options: SessionOptions) -> Session {
    let backend = SyntheticBackend::new(scene.clone());
    let camera = backend.list().unwrap().remove(0);
    let stream = backend.open(&camera, settings).unwrap();
    Session::start(stream, scene.aspect(), options)
}

fn calibrate(
    scene: &SyntheticScene,
    options: SessionOptions,
) -> (Calibration, Truth, Matrix3<f64>) {
    let mut session = open(scene, &CaptureSettings::default(), options);
    let preview = session.preview().unwrap();
    assert_eq!(
        preview.hint,
        Hint::Ready,
        "preview: {} of {} dots",
        preview.dots_matched,
        preview.dots_expected
    );

    let mut steps = Vec::new();
    session.measure_screen(&mut |s| steps.push(s)).unwrap();
    assert!(matches!(
        steps.first(),
        Some(Step::Capturing { frame: 0, .. })
    ));
    assert_eq!(steps.last(), Some(&Step::Done));

    let marker_preview = session.preview_markers().unwrap();
    assert_eq!(
        marker_preview.hint,
        MarkerHint::Ready,
        "{:?}",
        marker_preview.blobs
    );
    assert!(marker_preview.matched.is_some());
    session.measure_markers(&mut |_| {}).unwrap();

    let alignment = if scene.imu_enabled {
        scene.camera_to_aligned()
    } else {
        Matrix3::identity()
    };
    (session.finish().unwrap(), scene.truth(), alignment)
}

/// Compares everything the hub consumes with the ground truth.
fn assert_matches_truth(cal: &Calibration, truth: &Truth, alignment: &Matrix3<f64>, tol_m: f64) {
    let expected = truth.calibration(alignment);
    let got = &cal.calibration;

    // bounds() order is tl, tr, bl, br.
    let corners = truth.corners_m();
    let truth_bounds = [corners[0], corners[1], corners[3], corners[2]];
    for (g, t) in got.bounds().iter().zip(truth_bounds) {
        let (gx, gy) = (f64::from(g.x), f64::from(g.y));
        let err = ((gx - t.x).powi(2) + (gy - t.y).powi(2)).sqrt();
        assert!(err < tol_m, "corner {g:?} vs {t:?}: {err} m");
    }

    let size = cal.review.size_m;
    for (got, want) in size.iter().zip(truth.size_m) {
        assert!(
            (got / want - 1.0).abs() < 0.005,
            "size {size:?} vs {:?}",
            truth.size_m
        );
    }
    assert!((cal.review.diagonal_in / truth.diagonal_in() - 1.0).abs() < 0.005);

    for (i, (g, e)) in got
        .object_points
        .iter()
        .zip(&expected.object_points)
        .enumerate()
    {
        let err = f64::from((g - e).norm());
        assert!(err < tol_m, "marker {i}: {g:?} vs {e:?} ({err} m)");
    }

    let angle = got.rotation.angle_to(&expected.rotation).to_degrees();
    assert!(angle < 0.5, "rotation off by {angle} degrees");

    // The homography maps meters to the unit square and the corners back.
    let inverse = got.homography.try_inverse().unwrap();
    let br = inverse.transform_point(&Point2::new(1.0, 1.0));
    assert!((f64::from(br.x) - truth.size_m[0]).abs() < tol_m);
}

#[test]
fn default_scene_is_recovered() {
    let scene = SyntheticScene::default();
    let (cal, truth, alignment) = calibrate(&scene, SessionOptions::default());
    assert_matches_truth(&cal, &truth, &alignment, 0.003);
    assert!(
        cal.review.plane_rmse_m < 0.004,
        "plane rmse {}",
        cal.review.plane_rmse_m
    );
    assert_eq!(cal.review.dots_used, cal.review.dots_expected);
    assert!(cal.review.gravity_aligned);
    // Markers project into the color image inside the screen outline.
    for p in cal.review.marker_px {
        assert!(p.x > 0.0 && p.x < 1280.0 && p.y > 0.0 && p.y < 720.0);
    }
}

#[test]
fn noisy_depth_with_holes_and_outliers() {
    let scene = SyntheticScene {
        depth_noise_m: 0.004,
        depth_hole_fraction: 0.12,
        depth_outlier_fraction: 0.05,
        depth_dropout_region: Some([0.55, 0.3, 0.8, 0.6]),
        color_noise: 4.0,
        ..SyntheticScene::default()
    };
    let (cal, truth, alignment) = calibrate(&scene, SessionOptions::default());
    assert_matches_truth(&cal, &truth, &alignment, 0.003);
}

#[test]
fn rotated_and_offset_camera() {
    let scene = SyntheticScene {
        screen_center_m: nalgebra::Vector3::new(0.45, -0.15, 2.3),
        screen_yaw_deg: -22.0,
        screen_pitch_deg: 4.0,
        camera_yaw_deg: 11.0,
        camera_pitch_deg: 9.0,
        camera_roll_deg: 7.0,
        ..SyntheticScene::default()
    };
    let (cal, truth, alignment) = calibrate(&scene, SessionOptions::default());
    assert_matches_truth(&cal, &truth, &alignment, 0.003);
}

#[test]
fn works_without_an_imu() {
    let scene = SyntheticScene {
        imu_enabled: false,
        camera_pitch_deg: 6.0,
        ..SyntheticScene::default()
    };
    let (cal, truth, alignment) = calibrate(&scene, SessionOptions::default());
    assert_eq!(alignment, Matrix3::identity());
    assert!(!cal.review.gravity_aligned);
    assert_matches_truth(&cal, &truth, &alignment, 0.003);
}

#[test]
fn clutter_away_from_the_screen_is_ignored() {
    let scene = SyntheticScene {
        // A lamp up in a corner and a reflection down in another, both well off the screen.
        ir_clutter: vec![Point2::new(100.0, 60.0), Point2::new(40.0, 360.0)],
        ..SyntheticScene::default()
    };
    let (cal, truth, alignment) = calibrate(&scene, SessionOptions::default());
    assert_matches_truth(&cal, &truth, &alignment, 0.003);
}

#[test]
fn a_seventh_bright_spot_on_the_screen_is_refused_with_its_count() {
    let scene = SyntheticScene {
        // Between the markers, on the screen: it cannot be told from a marker.
        ir_clutter: vec![Point2::new(300.0, 200.0)],
        ..SyntheticScene::default()
    };
    let mut session = open(
        &scene,
        &CaptureSettings::default(),
        SessionOptions::default(),
    );
    session.measure_screen(&mut |_| {}).unwrap();
    let preview = session.preview_markers().unwrap();
    assert_eq!(preview.hint, MarkerHint::WrongCount { near_screen: 7 });
    assert!(preview.matched.is_none());
    assert!(matches!(
        session.measure_markers(&mut |_| {}),
        Err(SessionError::Markers(MarkerError::WrongCount {
            near_screen: 7
        }))
    ));
}

#[test]
fn known_diagonal_rectifies_the_size() {
    let mut scene = SyntheticScene::default();
    // The physical screen is 55 inches; the solver gets the user's answer.
    let truth_diagonal = 55.0;
    let options = SessionOptions {
        known_size: Some(KnownSize::Diagonal {
            inches: truth_diagonal,
        }),
        ..SessionOptions::default()
    };
    let (cal, truth, alignment) = calibrate(&scene, options);
    assert!((cal.review.diagonal_in - truth_diagonal).abs() / truth_diagonal < 0.002);
    assert_matches_truth(&cal, &truth, &alignment, 0.003);

    // A wrong answer is honored: the result is scaled to it.
    scene.seed = 2;
    let options = SessionOptions {
        known_size: Some(KnownSize::Diagonal { inches: 60.0 }),
        ..SessionOptions::default()
    };
    let (cal, _, _) = calibrate(&scene, options);
    assert!((cal.review.diagonal_in - 60.0).abs() < 0.05);
}

#[test]
fn preview_explains_what_is_wrong() {
    let default = SyntheticScene::default();
    let settings = CaptureSettings::default();

    let mut blank = open(
        &SyntheticScene {
            show_pattern: false,
            ..default.clone()
        },
        &settings,
        SessionOptions::default(),
    );
    assert_eq!(blank.preview().unwrap().hint, Hint::ScreenNotFound);

    let dark_settings = CaptureSettings {
        color_exposure_us: 25,
        ..CaptureSettings::default()
    };
    let mut dark = open(&default, &dark_settings, SessionOptions::default());
    assert_eq!(dark.preview().unwrap().hint, Hint::TooDark);

    let bright_settings = CaptureSettings {
        color_exposure_us: 120_000,
        ..CaptureSettings::default()
    };
    let mut bright = open(&default, &bright_settings, SessionOptions::default());
    assert_eq!(bright.preview().unwrap().hint, Hint::TooBright);

    // Screen larger than the view: only part of it is visible.
    let wide = SyntheticScene {
        screen_size_m: [3.2, 1.8],
        ..default.clone()
    };
    let mut partial = open(&wide, &settings, SessionOptions::default());
    assert_eq!(partial.preview().unwrap().hint, Hint::ScreenPartlyVisible);

    let mut tilted = open(
        &SyntheticScene {
            camera_pitch_deg: 50.0,
            ..default.clone()
        },
        &settings,
        SessionOptions::default(),
    );
    // Looking down 50 degrees may lose the screen too; either way the user is told.
    assert_ne!(tilted.preview().unwrap().hint, Hint::Ready);

    let mut depthless = open(
        &SyntheticScene {
            depth_hole_fraction: 0.0,
            depth_dropout_region: Some([0.0, 0.0, 1.0, 1.0]),
            ..default.clone()
        },
        &settings,
        SessionOptions::default(),
    );
    assert_eq!(depthless.preview().unwrap().hint, Hint::DepthPoor);

    let mut near = open(
        &SyntheticScene {
            screen_size_m: [0.6, 0.34],
            screen_center_m: nalgebra::Vector3::new(0.0, 0.0, 0.45),
            ..default.clone()
        },
        &settings,
        SessionOptions::default(),
    );
    assert_eq!(near.preview().unwrap().hint, Hint::TooClose);

    let mut healthy = open(&default, &settings, SessionOptions::default());
    let p = healthy.preview().unwrap();
    assert_eq!(p.hint, Hint::Ready);
    assert!((p.distance_m.unwrap() - 2.0).abs() < 0.3);
    assert!(p.depth_coverage.unwrap() > 0.9);
    assert_eq!(p.dots_matched, p.dots_expected);
}

#[test]
fn measuring_without_a_pattern_fails_cleanly() {
    let mut session = open(
        &SyntheticScene {
            show_pattern: false,
            ..SyntheticScene::default()
        },
        &CaptureSettings::default(),
        SessionOptions::default(),
    );
    let err = session.measure_screen(&mut |_| {}).unwrap_err();
    assert!(
        matches!(err, SessionError::Solve(SolveError::PatternNotFound(_))),
        "{err}"
    );
    // The message says what the search saw, and the whole burst it searched is kept.
    assert!(err.to_string().contains("dot-like spots"), "{err}");
    let attempt = session.attempt_frames();
    assert_eq!(
        attempt.screen_burst.len(),
        SessionOptions::default().burst_frames
    );
    for frame in &attempt.screen_burst {
        assert_eq!(frame.color.width(), session.geometry().color.width);
    }
}

#[test]
fn an_attempt_keeps_every_frame_it_used() {
    let scene = SyntheticScene::default();
    let mut session = open(
        &scene,
        &CaptureSettings::default(),
        SessionOptions::default(),
    );
    session.measure_screen(&mut |_| {}).unwrap();
    session.preview_markers().unwrap();
    session.measure_markers(&mut |_| {}).unwrap();
    let attempt = session.attempt_frames();
    assert_eq!(
        attempt.screen_burst.len(),
        SessionOptions::default().burst_frames
    );
    assert_eq!(attempt.marker_burst_ir.len(), MARKER_FRAMES);
    assert!(attempt.marker_preview_ir.is_some());
    // A later failed measurement replaces the screen burst with the failed one.
    let mut blank = open(
        &SyntheticScene {
            show_pattern: false,
            ..scene
        },
        &CaptureSettings::default(),
        SessionOptions::default(),
    );
    assert!(blank.attempt_frames().screen_burst.is_empty());
    blank.measure_screen(&mut |_| {}).unwrap_err();
    assert!(!blank.attempt_frames().screen_burst.is_empty());
}

#[test]
fn markers_need_a_measured_screen_and_a_visible_layout() {
    let mut session = open(
        &SyntheticScene::default(),
        &CaptureSettings::default(),
        SessionOptions::default(),
    );
    assert!(matches!(
        session.measure_markers(&mut |_| {}),
        Err(SessionError::ScreenNotMeasured)
    ));
    assert!(matches!(
        session.finish(),
        Err(SessionError::ScreenNotMeasured)
    ));
    session.measure_screen(&mut |_| {}).unwrap();
    assert!(matches!(
        session.finish(),
        Err(SessionError::MarkersNotMeasured)
    ));
}

#[test]
fn infrared_camera_with_its_own_pose_and_lens() {
    use nalgebra::{Rotation3, Vector3};
    use odyssey_autocalib::camera::Extrinsics;

    let mut scene = SyntheticScene::default();
    scene.geometry.ir_to_depth = Extrinsics {
        rotation: Rotation3::from_euler_angles(0.01, -0.02, 0.005).into_inner(),
        translation: Vector3::new(0.045, 0.0, 0.0),
    };
    scene.geometry.ir.fx = 470.0;
    scene.geometry.ir.fy = 470.0;
    let (cal, truth, alignment) = calibrate(&scene, SessionOptions::default());
    assert_matches_truth(&cal, &truth, &alignment, 0.003);
}

#[test]
fn distant_and_noisy_screen() {
    let scene = SyntheticScene {
        screen_center_m: nalgebra::Vector3::new(0.0, 0.0, 4.0),
        screen_yaw_deg: 25.0,
        depth_noise_m: 0.006,
        ..SyntheticScene::default()
    };
    let (cal, truth, alignment) = calibrate(&scene, SessionOptions::default());
    assert_matches_truth(&cal, &truth, &alignment, 0.003);
}

fn steep_scene() -> SyntheticScene {
    SyntheticScene {
        screen_yaw_deg: 55.0,
        screen_center_m: nalgebra::Vector3::new(0.35, -0.1, 2.0),
        camera_yaw_deg: 6.0,
        camera_pitch_deg: 4.0,
        ..SyntheticScene::default()
    }
}

#[test]
fn steep_viewing_angle_is_recovered() {
    let (cal, truth, alignment) = calibrate(&steep_scene(), SessionOptions::default());
    assert_matches_truth(&cal, &truth, &alignment, 0.003);
}

/// Large dots at a steep angle have the strongest perspective bias between a
/// dot's centroid and its true center.
#[test]
fn steep_viewing_angle_with_large_dots_is_recovered() {
    use odyssey_autocalib::solve::{SolveOptions, solve_screen};

    let scene = SyntheticScene {
        dot_scale: 1.4,
        ..steep_scene()
    };
    let backend = SyntheticBackend::new(scene.clone());
    let camera = backend.list().unwrap().remove(0);
    let mut stream = backend.open(&camera, &CaptureSettings::default()).unwrap();
    let geometry = *stream.geometry();
    let frames: Vec<_> = (0..9)
        .map(|_| {
            stream
                .next_frames(std::time::Duration::from_secs(1))
                .unwrap()
        })
        .collect();
    let truth = scene.truth();

    let fit = solve_screen(
        &geometry,
        &scene.pattern(),
        &frames,
        &scene.camera_to_aligned(),
        &SolveOptions::default(),
    )
    .expect("solve");
    for (got, want) in fit.corners_m.iter().zip(truth.corners_m()) {
        assert!((got - want).norm() < 0.003, "corner {got:?} vs {want:?}");
    }
    for (got, want) in fit.size_m.iter().zip(truth.size_m) {
        assert!((got / want - 1.0).abs() < 0.005);
    }
}

fn panel_size(scene: &SyntheticScene) -> KnownSize {
    KnownSize::Dimensions {
        width_m: scene.screen_size_m[0],
        height_m: scene.screen_size_m[1],
    }
}

fn size_error(cal: &Calibration, truth: &Truth) -> f64 {
    let [w, h] = cal.review.size_m;
    (w / truth.size_m[0] - 1.0)
        .abs()
        .max((h / truth.size_m[1] - 1.0).abs())
}

#[test]
fn a_known_panel_size_removes_depth_scale_error() {
    let scene = SyntheticScene {
        depth_scale_error: 0.01,
        ..SyntheticScene::default()
    };

    let (unanchored, truth, alignment) = calibrate(&scene, SessionOptions::default());
    let drift = size_error(&unanchored, &truth);
    eprintln!(
        "1% depth scale error, depth only: size error {:.3}%",
        drift * 100.0
    );
    assert!((0.007..0.013).contains(&drift), "size error {drift}");
    assert_eq!(unanchored.review.size_source, SizeSource::Measured);
    assert!(unanchored.review.size_check.is_none());

    let options = SessionOptions {
        known_size: Some(panel_size(&scene)),
        ..SessionOptions::default()
    };
    let (anchored, truth, alignment_anchored) = calibrate(&scene, options);
    let residual = size_error(&anchored, &truth);
    eprintln!(
        "1% depth scale error, panel size known: size error {:.3}%",
        residual * 100.0
    );
    assert!(residual < 0.002, "size error {residual}");
    assert_matches_truth(&anchored, &truth, &alignment_anchored, 0.003);
    assert_eq!(anchored.review.size_source, SizeSource::KnownDimensions);

    // The depth measurement is off by the scale error, and the check says so
    // without crossing the disagreement threshold.
    let check = anchored.review.size_check.expect("size check");
    assert!(
        (check.ratio_w - 1.01).abs() < 0.004 && (check.ratio_h - 1.01).abs() < 0.004,
        "{check:?}"
    );
    assert!(!check.disagrees());
    let _ = alignment;
}

#[test]
fn a_declared_size_with_the_wrong_aspect_is_refused() {
    let scene = SyntheticScene::default();
    let wrong = KnownSize::Dimensions {
        width_m: scene.screen_size_m[0],
        height_m: scene.screen_size_m[1] * 1.04,
    };
    let mut session = open(
        &scene,
        &CaptureSettings::default(),
        SessionOptions {
            known_size: Some(wrong),
            ..SessionOptions::default()
        },
    );
    let error = session.measure_screen(&mut |_| {}).unwrap_err();
    assert!(
        matches!(
            error,
            SessionError::Solve(SolveError::KnownSizeAspectMismatch { .. })
        ),
        "{error}"
    );

    // 1% off is within the tolerance and is stretched onto the pattern.
    let close = KnownSize::Dimensions {
        width_m: scene.screen_size_m[0],
        height_m: scene.screen_size_m[1] * 1.01,
    };
    let mut session = open(
        &scene,
        &CaptureSettings::default(),
        SessionOptions {
            known_size: Some(close),
            ..SessionOptions::default()
        },
    );
    session.measure_screen(&mut |_| {}).unwrap();
}

#[test]
fn overscan_blocks_readiness_only_when_edges_are_required() {
    let settings = CaptureSettings::default();
    let required = SessionOptions {
        require_visible_edges: true,
        ..SessionOptions::default()
    };

    let mut flat = open(&SyntheticScene::default(), &settings, required.clone());
    let preview = flat.preview().unwrap();
    assert_eq!(preview.edges, EdgeCheck::Visible);
    assert_eq!(preview.hint, Hint::Ready);
    assert!(preview.is_ready());

    let overscanned = SyntheticScene {
        overscan_fraction: 0.03,
        ..SyntheticScene::default()
    };
    let mut blocked = open(&overscanned, &settings, required);
    let preview = blocked.preview().unwrap();
    assert!(
        matches!(preview.edges, EdgeCheck::Cropped { sides } if sides.count() == 4),
        "{:?}",
        preview.edges
    );
    assert_eq!(preview.hint, Hint::EdgesCropped);
    assert!(!preview.is_ready());
    let error = blocked.measure_screen(&mut |_| {}).unwrap_err();
    assert!(
        matches!(error, SessionError::EdgesCropped(sides) if sides.count() == 4),
        "{error}"
    );

    // A projector-style run reports the crop but is never held up by it.
    let mut projector = open(&overscanned, &settings, SessionOptions::default());
    let preview = projector.preview().unwrap();
    assert!(matches!(preview.edges, EdgeCheck::Cropped { .. }));
    assert_eq!(preview.hint, Hint::Ready);
    assert!(projector.measure_screen(&mut |_| {}).is_ok());
}

#[test]
fn small_overscan_is_still_noticed() {
    let mut session = open(
        &SyntheticScene {
            overscan_fraction: 0.012,
            ..SyntheticScene::default()
        },
        &CaptureSettings::default(),
        SessionOptions {
            require_visible_edges: true,
            ..SessionOptions::default()
        },
    );
    assert_eq!(session.preview().unwrap().hint, Hint::EdgesCropped);
}

#[test]
fn overscan_without_a_known_size_measures_the_displayed_image() {
    let scene = SyntheticScene {
        overscan_fraction: 0.03,
        ..SyntheticScene::default()
    };
    let (cal, truth, alignment) = calibrate(&scene, SessionOptions::default());
    assert_matches_truth(&cal, &truth, &alignment, 0.003);
    // The measured size is the image, 3% larger than the panel.
    assert!((cal.review.size_m[0] / scene.screen_size_m[0] - 1.03).abs() < 0.004);
}

#[test]
fn overscan_makes_a_panel_size_disagree_with_the_measurement() {
    let scene = SyntheticScene {
        overscan_fraction: 0.03,
        ..SyntheticScene::default()
    };
    let options = SessionOptions {
        known_size: Some(panel_size(&scene)),
        ..SessionOptions::default()
    };
    let (cal, _, _) = calibrate(&scene, options);
    let check = cal.review.size_check.expect("size check");
    eprintln!(
        "3% overscan against the panel size: ratios {:.4} x {:.4}",
        check.ratio_w, check.ratio_h
    );
    assert!(check.disagrees(), "{check:?}");
    assert!(
        (check.ratio_w - 1.03).abs() < 0.004 && (check.ratio_h - 1.03).abs() < 0.004,
        "{check:?}"
    );

    // Without overscan the same declaration agrees.
    let clean = SyntheticScene::default();
    let options = SessionOptions {
        known_size: Some(panel_size(&clean)),
        ..SessionOptions::default()
    };
    let (cal, _, _) = calibrate(&clean, options);
    let check = cal.review.size_check.expect("size check");
    assert!(!check.disagrees(), "{check:?}");
    assert!((check.ratio_w - 1.0).abs() < 0.003, "{check:?}");
}

/// The ring must read as visible, never as cropped, wherever the screen is
/// shown completely; otherwise a flat-screen user would be blocked for no reason.
#[test]
fn an_uncropped_ring_is_never_reported_cropped() {
    let default = SyntheticScene::default();
    let scenes = [
        ("default", default.clone()),
        ("steep", steep_scene()),
        (
            "far",
            SyntheticScene {
                screen_center_m: nalgebra::Vector3::new(0.0, 0.0, 4.0),
                screen_yaw_deg: 25.0,
                ..default.clone()
            },
        ),
        (
            "noisy",
            SyntheticScene {
                color_noise: 6.0,
                depth_noise_m: 0.004,
                ..default.clone()
            },
        ),
        (
            "rolled",
            SyntheticScene {
                camera_roll_deg: 7.0,
                camera_yaw_deg: 11.0,
                camera_pitch_deg: 9.0,
                screen_yaw_deg: -22.0,
                ..default.clone()
            },
        ),
    ];
    for (name, scene) in scenes {
        let mut session = open(
            &scene,
            &CaptureSettings::default(),
            SessionOptions::default(),
        );
        let preview = session.preview().unwrap();
        eprintln!("{name}: edges {:?}, hint {:?}", preview.edges, preview.hint);
        assert!(
            !matches!(preview.edges, EdgeCheck::Cropped { .. }),
            "{name}: {:?}",
            preview.edges
        );
        // At 55 degrees of yaw, or 4 m away on a 720p camera, the border is
        // barely a pixel wide, which the check declines to judge instead of guessing.
        if !matches!(name, "steep" | "far") {
            assert_eq!(preview.edges, EdgeCheck::Visible, "{name}");
        }
    }
}

#[test]
fn finish_measured_drops_the_declared_size_without_the_camera() {
    let scene = SyntheticScene::default();
    // A diagonal 40% too small: the declared size disagrees with the depth
    // measurement, which is what the review offers to override.
    let wrong = KnownSize::Diagonal {
        inches: scene.truth().diagonal_in() * 0.6,
    };
    let mut session = open(
        &scene,
        &CaptureSettings::default(),
        SessionOptions {
            known_size: Some(wrong),
            ..SessionOptions::default()
        },
    );
    session.preview().unwrap();
    session.measure_screen(&mut |_| {}).unwrap();
    session.preview_markers().unwrap();
    session.measure_markers(&mut |_| {}).unwrap();

    let declared = session.finish().unwrap();
    assert_eq!(declared.review.size_source, SizeSource::KnownDiagonal);
    assert!(declared.review.size_check.is_some_and(|c| c.disagrees()));

    let measured = session.finish_measured().unwrap();
    assert_eq!(measured.review.size_source, SizeSource::Measured);
    assert!(measured.review.size_check.is_none());
    let truth = scene.truth();
    assert!(
        size_error(&measured, &truth) < 0.01,
        "size error {}",
        size_error(&measured, &truth)
    );
    // Markers follow the new fit: they sit at their layout positions on the real screen.
    assert_matches_truth(&measured, &truth, &scene.camera_to_aligned(), 0.012);
}

#[test]
fn finish_measured_without_a_declared_size_is_finish() {
    let scene = SyntheticScene::default();
    let mut session = open(
        &scene,
        &CaptureSettings::default(),
        SessionOptions::default(),
    );
    session.preview().unwrap();
    session.measure_screen(&mut |_| {}).unwrap();
    session.preview_markers().unwrap();
    session.measure_markers(&mut |_| {}).unwrap();
    assert_eq!(
        session.finish().unwrap().calibration.homography,
        session.finish_measured().unwrap().calibration.homography
    );
}
