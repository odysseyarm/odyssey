//! Synthetic engineering regressions, not physical captures or hardware accuracy specifications.

use nalgebra::{Quaternion, UnitQuaternion, Vector3};
use odyssey_sim::dataset::{
    SCENE_SCHEMA_VERSION, SceneCamera, SceneCoordinateBasis, SceneDescription, SceneFrame,
    ScenePose, SceneScreen,
};
use odyssey_usb::device::GeneralSettings;
use opencv_ros_camera::RosOpenCvIntrinsics;

use super::{Input, Provenance, SCHEMA_VERSION, Sample, Screen, Source, Thresholds, Trace, Truth};

const WIDTH_M: f64 = 1.2;
const HEIGHT_M: f64 = 0.675;
const SOURCE_ID: &str = "synthetic-unit-0";
const CAMERA_ID: &str = "synthetic-unit-0-nf";
const FRAME_COUNT: usize = 140;

#[derive(Clone, Copy)]
enum Trajectory {
    Stationary([f64; 2]),
    StationaryRoll,
    Motion,
    OpticalLoss,
    ResetRecovery,
    TimestampWrap,
    IrregularNoise,
    OffAxis,
}

/// Fixed-seed raw observations projected from independent camera poses and scene geometry.
/// Limits guard engineering regressions only; no physical performance is asserted.
pub fn builtin_scenarios() -> Vec<Trace> {
    [
        ("stationary-center", Trajectory::Stationary([0.5, 0.5])),
        ("stationary-left", Trajectory::Stationary([0.08, 0.5])),
        ("stationary-right", Trajectory::Stationary([0.92, 0.5])),
        ("stationary-top", Trajectory::Stationary([0.5, 0.08])),
        ("stationary-bottom", Trajectory::Stationary([0.5, 0.92])),
        ("stationary-roll", Trajectory::StationaryRoll),
        ("smooth-motion", Trajectory::Motion),
        ("optical-loss-reacquisition", Trajectory::OpticalLoss),
        ("reset-outage-reacquisition", Trajectory::ResetRecovery),
        ("sensor-timestamp-wrap", Trajectory::TimestampWrap),
        ("irregular-timing-noise", Trajectory::IrregularNoise),
    ]
    .into_iter()
    .map(|(name, trajectory)| generate(name, trajectory))
    .collect()
}

const SURFACE_CASES: [&str; 4] = [
    "calibration-plane",
    "projective-plane",
    "wavy-grid",
    "nonlinear-projector-grid",
];

/// Authored display-mapping cases. Truth is mesh-derived and markers sit on the
/// surface at their calibrated UVs; the measured surface travels in the
/// tracker's own `ScreenCalibration`, so the tracker maps aim through it.
pub fn surface_cases() -> Vec<Trace> {
    SURFACE_CASES
        .into_iter()
        .map(|name| {
            let trajectory = if name == "wavy-grid" {
                Trajectory::OffAxis
            } else {
                Trajectory::Stationary([0.32, 0.37])
            };
            let mut trace = generate(name, trajectory);
            trace.provenance.description = format!(
                "Deterministic {name} display case; mesh-derived truth, markers on the surface, measured surface in the tracker calibration, no physical capture."
            );
            trace
        })
        .collect()
}

pub fn scene_cases() -> Vec<(String, SceneDescription)> {
    surface_cases()
        .into_iter()
        .map(|trace| {
            let mut scene = trace.scene.expect("authored scene");
            scene.provenance = format!(
                "Deterministic {} display geometry; mesh-derived labels, instantaneous exposure, no physical capture.",
                trace.name
            );
            scene.relabel().expect("authored geometry labels");
            (trace.name, scene)
        })
        .collect()
}

/// Shapes the display for the named surface case; any other name keeps the plane.
fn apply_display(name: &str, screen: &mut SceneScreen) {
    if name == "projective-plane" {
        let unit_to_screen = nalgebra::Matrix3::new(
            WIDTH_M, 0.08, -0.04, 0.03, HEIGHT_M, -0.02, 0.22, -0.08, 1.0,
        );
        screen.calibration.homography =
            unit_to_screen.try_inverse().expect("projective fit").cast();
    } else if name.ends_with("grid") {
        let columns = 9;
        let rows = 7;
        let nonlinear = name == "nonlinear-projector-grid";
        let mut positions_m = Vec::with_capacity((columns * rows) as usize);
        for row in 0..rows {
            for column in 0..columns {
                let u = f64::from(column) / f64::from(columns - 1);
                let v = f64::from(row) / f64::from(rows - 1);
                let tau = std::f64::consts::TAU;
                let pi = std::f64::consts::PI;
                positions_m.push([
                    WIDTH_M
                        * (u + if nonlinear {
                            0.12 * (tau * u).sin()
                        } else {
                            0.0
                        }),
                    HEIGHT_M
                        * (v + if nonlinear {
                            0.08 * (tau * v).sin()
                        } else {
                            0.0
                        }),
                    if nonlinear {
                        0.0
                    } else {
                        0.12 * (pi * u).sin() * (pi * v).sin()
                    },
                ]);
            }
        }
        // Markers are drawn at their planar-fit UVs, so they land where the surface puts them.
        let homography = screen.calibration.homography.cast::<f64>();
        screen.calibration.surface = Some(odyssey_common::ScreenSurface {
            columns,
            rows,
            positions_m,
        });
        let on_surface = {
            let geometry = screen.geometry().expect("authored grid");
            screen.calibration.object_points.map(|p| {
                let uv = homography * Vector3::new(f64::from(p.x), f64::from(p.y), 1.0);
                let world = geometry
                    .point_at_uv([uv.x / uv.z, uv.y / uv.z])
                    .expect("marker UV inside display");
                geometry.world_to_screen(world).map(|v| v as f32)
            })
        };
        for (point, measured) in screen.calibration.object_points.iter_mut().zip(on_surface) {
            *point = measured.into();
        }
    }
}

fn generate(name: &str, trajectory: Trajectory) -> Trace {
    let mut screen = SceneScreen {
        id: 0,
        calibration: odyssey_sim::synthetic_screen_calibration(),
        marker_ids: std::array::from_fn(|index| format!("screen-0-marker-{index}")),
    };
    apply_display(name, &mut screen);
    let calibration = screen.calibration.clone();
    let camera = SceneCamera {
        id: CAMERA_ID.into(),
        source_id: SOURCE_ID.into(),
        image_size_px: [640, 480],
        intrinsics: [[500.0, 0.0, 320.0], [0.0, 500.0, 240.0], [0.0, 0.0, 1.0]],
        distortion: [0.0; 5],
        sensor: None,
    };
    let optics = RosOpenCvIntrinsics::from_params(500.0, 0.0, 500.0, 320.0, 240.0);
    let settings = GeneralSettings {
        camera_model_nf: optics.clone(),
        camera_model_wf: optics,
        ..GeneralSettings::default()
    };
    let mut frames = Vec::with_capacity(FRAME_COUNT);
    let mut samples = Vec::with_capacity(FRAME_COUNT * 2);
    let mut elapsed_us = 0_u64;
    let mut noise = Perturbation(0x0d15_ea5e_1234_5678);
    for index in 0..FRAME_COUNT {
        if index != 0 {
            elapsed_us += if matches!(trajectory, Trajectory::IrregularNoise) {
                [7_000, 13_000, 9_000, 11_000, 0, 17_000, 6_000, 10_000][index % 8]
            } else {
                10_000
            };
        }
        let (pose, acceleration) = camera_pose(trajectory, elapsed_us);
        let truth = truth_from_pose(&screen, &pose);
        let projected = project(&screen, &camera, &pose);
        let visible_marker_ids = screen
            .marker_ids
            .iter()
            .zip(&projected)
            .filter(|(_, pixel)| pixel.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        // Wire axes become negated xzy; rotate proper acceleration into camera coordinates first.
        let proper_acceleration = orientation(&pose).inverse_transform_vector(
            &(Vector3::from(acceleration) - Vector3::new(0.0, 9.81, 0.0)),
        );
        let mut accel = [
            -proper_acceleration.x as f32,
            -proper_acceleration.z as f32,
            -proper_acceleration.y as f32,
        ];
        frames.push(SceneFrame {
            id: format!("{name}-frame-{index:04}"),
            elapsed_us,
            exposure_us: 0,
            camera_id: CAMERA_ID.into(),
            camera_to_world: pose,
            visible_marker_ids,
            truth: truth.clone(),
        });
        let is_loss = matches!(trajectory, Trajectory::OpticalLoss);
        let reset_recovery = matches!(trajectory, Trajectory::ResetRecovery);
        let initial_blackout = is_loss && index < 20;
        let reset_blackout = reset_recovery && (40..50).contains(&index);
        let unavailable = initial_blackout || reset_blackout;
        let dropped = unavailable || (is_loss && (60..80).contains(&index));
        let score = is_loss || index >= 20;
        if reset_recovery && index == 40 {
            samples.push(Sample {
                elapsed_us,
                source_id: SOURCE_ID.into(),
                input: Input::Reset,
                truth: None,
                expectation: None,
                score,
                image_artifacts: Vec::new(),
            });
        }
        let timestamp_origin = if matches!(trajectory, Trajectory::TimestampWrap) {
            u32::MAX - 65_535
        } else {
            1_000
        };
        if matches!(trajectory, Trajectory::IrregularNoise) {
            for value in &mut accel {
                *value += (noise.next() * 0.002) as f32;
            }
        }
        let imu_valid =
            !unavailable && !(is_loss && index == 20) && !(reset_recovery && index == 50);
        samples.push(Sample {
            elapsed_us,
            source_id: SOURCE_ID.into(),
            input: Input::Imu {
                timestamp: timestamp_origin.wrapping_add(elapsed_us as u32),
                accel,
                gyro: [0.0; 3],
            },
            truth: imu_valid.then(|| truth.clone()),
            expectation: Some(imu_valid),
            score,
            image_artifacts: Vec::new(),
        });
        let mut points = [[0; 2]; 16];
        if !dropped {
            for (output, pixel) in points.iter_mut().zip(projected) {
                if let Some(mut pixel) = pixel {
                    if matches!(trajectory, Trajectory::IrregularNoise) {
                        pixel[0] += noise.next() * 0.6;
                        pixel[1] += noise.next() * 0.6;
                    }
                    *output = [pixel[0].round() as u16, pixel[1].round() as u16];
                }
            }
        }
        samples.push(Sample {
            elapsed_us,
            source_id: SOURCE_ID.into(),
            input: Input::Poc { points },
            truth: (!unavailable).then_some(truth),
            expectation: Some(!unavailable),
            score,
            image_artifacts: Vec::new(),
        });
    }
    Trace {
        schema_version: SCHEMA_VERSION,
        name: name.into(),
        provenance: Provenance {
            description: "Synthetic pinhole observations with independent trajectory truth; limits are engineering regressions, not hardware specifications.".into(),
            generator: Some("odyssey tracking-regression synthetic scene v1; perturbation seed 0x0d15ea5e12345678".into()),
        },
        scene: Some(SceneDescription {
            schema_version: SCENE_SCHEMA_VERSION,
            provenance: "Synthetic calibrated marker plane; instantaneous exposures; no physical capture or realistic image renderer.".into(),
            coordinate_basis: SceneCoordinateBasis::ScreenXRightYDownZForward,
            screens: vec![screen],
            cameras: vec![camera],
            frames,
        }),
        sources: vec![Source {
            id: SOURCE_ID.into(),
            device_uuid: [0x53, 0x59, 0x4e, 0, 0, 1],
            clock_basis: "Synthetic arrival microseconds from trace start; sensor IMU microseconds use separate stated offset (1000, or 4294901760 in wrap scenario), modulo 2^32; equal-time array order is IMU then optics. Optical reports have no sensor timestamp.".into(),
            settings,
        }],
        screens: vec![Screen { id: 0, calibration }],
        samples,
        thresholds: Thresholds {
            min_availability: 0.99,
            min_truth_matches: 200,
            max_aim_rmse: Some(0.02),
            max_position_rmse_m: Some(0.04),
            max_jump: Some(0.03),
            max_reacquisition_us: Some(10_000),
        },
    }
}

fn camera_pose(trajectory: Trajectory, elapsed_us: u64) -> (ScenePose, [f64; 3]) {
    let mut aim = match trajectory {
        Trajectory::Stationary(aim) => aim,
        _ => [0.5, 0.5],
    };
    let mut z = -2.0;
    let mut acceleration = [0.0; 3];
    if matches!(trajectory, Trajectory::Motion) {
        let time = elapsed_us.saturating_sub(200_000) as f64 / 1_000_000.0;
        aim[0] += 0.06 * (1.0 - (2.0 * time).cos());
        aim[1] += 0.06 * (1.0 - (1.3 * time).cos());
        z += 0.03 * (1.0 - (1.1 * time).cos());
        if elapsed_us > 200_000 {
            acceleration = [
                WIDTH_M * 0.06 * 4.0 * (2.0 * time).cos(),
                HEIGHT_M * 0.06 * 1.69 * (1.3 * time).cos(),
                0.03 * 1.21 * (1.1 * time).cos(),
            ];
        }
    }
    let mut pose = ScenePose {
        position_m: [aim[0] * WIDTH_M, aim[1] * HEIGHT_M, z],
        orientation_xyzw: if matches!(trajectory, Trajectory::StationaryRoll) {
            [0.0, 0.0, 0.02_f64.sin(), 0.02_f64.cos()]
        } else {
            [0.0, 0.0, 0.0, 1.0]
        },
    };
    if matches!(trajectory, Trajectory::OffAxis) {
        pose.position_m = [0.18, HEIGHT_M * 0.37, -2.0];
        pose.orientation_xyzw = [0.0, 0.09_f64.sin(), 0.0, 0.09_f64.cos()];
    }
    (pose, acceleration)
}

fn orientation(pose: &ScenePose) -> UnitQuaternion<f64> {
    let [x, y, z, w] = pose.orientation_xyzw;
    UnitQuaternion::new_normalize(Quaternion::new(w, x, y, z))
}

fn truth_from_pose(screen: &SceneScreen, pose: &ScenePose) -> Truth {
    let [x, y, z] = pose.position_m;
    let forward: [f64; 3] = orientation(pose).transform_vector(&Vector3::z()).into();
    let hit = screen
        .geometry()
        .expect("authored screen")
        .raycast(pose.position_m, forward)
        .expect("finite camera ray")
        .expect("authored camera aims at display");
    Truth {
        aimpoint: hit.uv,
        position_m: Some([x, -y, -z]),
        screen_id: u32::from(screen.id),
    }
}

fn project(screen: &SceneScreen, camera: &SceneCamera, pose: &ScenePose) -> [Option<[f64; 2]>; 6] {
    let geometry = screen.geometry().expect("authored screen");
    std::array::from_fn(|index| {
        let marker = geometry
            .marker_world(index)
            .expect("canonical marker index");
        let [u, v] = camera.project_world(pose, marker)?;
        ((1.0..=f64::from(camera.image_size_px[0] - 1)).contains(&u)
            && (1.0..=f64::from(camera.image_size_px[1] - 1)).contains(&v))
        .then_some([u, v])
    })
}

/// Fixed-seed LCG; output is uniform in [-1, 1].
pub(super) struct Perturbation(pub(super) u64);

impl Perturbation {
    pub(super) fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        f64::from((self.0 >> 32) as u32) / f64::from(u32::MAX) * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracking_regression::{AccuracyStatus, evaluate, replay};

    #[tokio::test]
    async fn independent_synthetic_truth_bounds_production_tracking() {
        for trace in builtin_scenarios() {
            let report = replay(&trace).await.expect("valid synthetic trace");
            assert!(
                report.passed,
                "{}: {:?}; {:?}",
                trace.name, report.violations, report.metrics
            );
            assert_eq!(report.accuracy_status, AccuracyStatus::Checked);
            assert!(report.metrics.truth_matches >= 200, "{}", trace.name);
            assert!(report.metrics.position_matches >= 200, "{}", trace.name);
            if trace.name == "reset-outage-reacquisition" {
                let reset = trace
                    .samples
                    .iter()
                    .position(|sample| matches!(sample.input, Input::Reset))
                    .expect("reset event");
                assert!(
                    report.capture.outputs[reset..reset + 22]
                        .iter()
                        .all(|output| output.tracking.is_none())
                );
                let recovered = report.capture.outputs[reset + 22]
                    .tracking
                    .as_ref()
                    .expect("optical recovery");
                assert!((recovered.aimpoint[0] - 0.5).abs() < 0.02);
                assert!((recovered.position_m[2] - 2.0).abs() < 0.04);
                assert_eq!(report.metrics.unreacquired_segments, 0);
                assert!(
                    report
                        .metrics
                        .max_reacquisition_us
                        .expect("reacquisition metric")
                        <= 10_000
                );
            }
        }
    }

    #[tokio::test]
    async fn finite_wrong_candidate_cannot_pass_absolute_accuracy_limits() {
        let trace = builtin_scenarios().remove(0);
        let baseline = replay(&trace).await.expect("baseline trace");
        assert!(baseline.passed, "{:?}", baseline.violations);
        let mut wrong_aim = baseline.capture.clone();
        for output in &mut wrong_aim.outputs {
            if let Some(tracking) = &mut output.tracking {
                tracking.aimpoint[0] += 0.25;
            }
        }
        let aim_report = evaluate(&trace, &wrong_aim).expect("finite candidate report");
        assert!(!aim_report.passed);
        assert!(aim_report.metrics.aim_rmse.expect("aim accuracy measured") > 0.2);
        let mut wrong_position = baseline.capture;
        for output in &mut wrong_position.outputs {
            if let Some(tracking) = &mut output.tracking {
                tracking.position_m[2] += 0.5;
            }
        }
        let position_report = evaluate(&trace, &wrong_position).expect("finite candidate report");
        assert!(!position_report.passed);
        assert!(
            position_report
                .metrics
                .position_rmse_m
                .expect("position accuracy measured")
                > 0.4
        );
    }

    #[test]
    fn raw_fixture_covers_wrap_loss_and_timing_without_changing_scene_labels() {
        let traces = builtin_scenarios();
        let wrap = traces
            .iter()
            .find(|trace| trace.name == "sensor-timestamp-wrap")
            .expect("wrap scenario");
        let timestamps: Vec<_> = wrap
            .samples
            .iter()
            .filter_map(|sample| match sample.input {
                Input::Imu { timestamp, .. } => Some(timestamp),
                _ => None,
            })
            .collect();
        assert!(timestamps.windows(2).any(|pair| pair[1] < pair[0]));
        let loss = traces
            .iter()
            .find(|trace| trace.name == "optical-loss-reacquisition")
            .expect("loss scenario");
        let scene = loss.scene.as_ref().expect("renderer labels");
        assert_eq!(scene.frames[60].visible_marker_ids.len(), 6);
        assert!(matches!(loss.samples[121].input, Input::Poc { points } if points == [[0; 2]; 16]));
        assert!(matches!(loss.samples[161].input, Input::Poc { points } if points != [[0; 2]; 16]));
        let noisy = traces
            .iter()
            .find(|trace| trace.name == "irregular-timing-noise")
            .expect("noise scenario");
        let frames = &noisy.scene.as_ref().expect("scene").frames;
        assert!(
            frames
                .windows(2)
                .any(|pair| pair[0].elapsed_us == pair[1].elapsed_us)
        );
        let distinct_intervals: std::collections::HashSet<_> = frames
            .windows(2)
            .map(|pair| pair[1].elapsed_us - pair[0].elapsed_us)
            .collect();
        assert!(distinct_intervals.len() > 3);
    }

    #[test]
    fn sensor_model_bounds_are_enforced() {
        use odyssey_sim::dataset::{DatasetError, SceneSensor};

        let scene = builtin_scenarios().remove(0).scene.expect("scene");
        let valid = SceneSensor {
            blur_sigma_px: 0.0,
            full_well_electrons: 1.0,
            read_noise_electrons: 0.0,
            bit_depth: 16,
            seed: 0,
        };
        let with = |sensor: SceneSensor| {
            let mut scene = scene.clone();
            scene.cameras[0].sensor = Some(sensor);
            scene.validate()
        };
        assert_eq!(with(valid), Ok(()));
        assert_eq!(
            with(SceneSensor {
                bit_depth: 1,
                ..valid
            }),
            Ok(())
        );
        for invalid in [
            SceneSensor {
                bit_depth: 0,
                ..valid
            },
            SceneSensor {
                bit_depth: 17,
                ..valid
            },
            SceneSensor {
                full_well_electrons: 0.999,
                ..valid
            },
            SceneSensor {
                read_noise_electrons: -0.1,
                ..valid
            },
            SceneSensor {
                blur_sigma_px: f64::NAN,
                ..valid
            },
            SceneSensor {
                blur_sigma_px: f64::INFINITY,
                ..valid
            },
        ] {
            assert_eq!(with(invalid), Err(DatasetError::Camera), "{invalid:?}");
        }
    }

    #[test]
    fn renderer_labels_reject_invalid_pose_references_and_exposure() {
        use odyssey_sim::dataset::DatasetError;

        let scene = builtin_scenarios().remove(0).scene.expect("scene");
        let mut invalid = scene.clone();
        invalid.frames[0].camera_id = "missing-camera".into();
        assert_eq!(invalid.validate(), Err(DatasetError::Frame));
        let mut invalid = scene.clone();
        invalid.frames[0].camera_to_world.orientation_xyzw = [0.0; 4];
        assert_eq!(invalid.validate(), Err(DatasetError::Frame));
        let mut invalid = scene.clone();
        invalid.frames[1].exposure_us = u64::MAX;
        assert_eq!(invalid.validate(), Err(DatasetError::Frame));
        let mut invalid = scene;
        invalid.cameras[0].intrinsics[0][0] = f64::NAN;
        assert_eq!(invalid.validate(), Err(DatasetError::Camera));
    }

    #[test]
    fn authored_grid_cases_expose_planar_fit_error_without_relaxing_production_limits() {
        let cases = scene_cases();
        for (name, scene) in cases {
            scene.validate().expect("valid authored scene");
            let screen = &scene.screens[0];
            let frame = &scene.frames[0];
            let direction = orientation(&frame.camera_to_world)
                .transform_vector(&Vector3::z())
                .into();
            let hit = screen
                .geometry()
                .unwrap()
                .raycast(frame.camera_to_world.position_m, direction)
                .unwrap()
                .unwrap();
            assert_eq!(frame.truth.aimpoint, hit.uv);
            if name.ends_with("grid") {
                let mut planar = screen.clone();
                planar.calibration.surface = None;
                let fit = planar
                    .geometry()
                    .unwrap()
                    .raycast(frame.camera_to_world.position_m, direction)
                    .unwrap()
                    .unwrap();
                assert!(
                    (fit.uv[0] - hit.uv[0]).abs() > 0.005,
                    "{name}: planar={:?}, mesh={:?}",
                    fit.uv,
                    hit.uv
                );
            }
        }
    }

    #[tokio::test]
    async fn tracker_maps_aim_through_measured_display_surface() {
        for trace in surface_cases() {
            let report = replay(&trace).await.expect("valid surface trace");
            assert!(report.passed, "{}: {:?}", trace.name, report.violations);
            if !trace.name.ends_with("grid") {
                continue;
            }
            // Same observations and truth, but the tracker only knows the planar fit.
            let mut planar = trace.clone();
            planar.screens[0].calibration.surface = None;
            if let Some(scene) = &mut planar.scene {
                scene.screens[0].calibration.surface = None;
            }
            let planar = replay(&planar).await.expect("valid planar trace");
            let measured = report.metrics.aim_rmse.expect("aim measured");
            let fitted = planar.metrics.aim_rmse.expect("aim measured");
            assert!(
                fitted > 2.0 * measured,
                "{}: surface {measured}, planar {fitted}",
                trace.name
            );
            if trace.name == "nonlinear-projector-grid" {
                assert!(!planar.passed, "planar fit must miss the default aim limit");
            }
        }
    }

    #[test]
    fn scene_and_tracker_calibration_disagreement_is_rejected() {
        let mut trace = builtin_scenarios().remove(0);
        trace.validate().expect("canonical calibration shared");
        trace.scene.as_mut().unwrap().screens[0]
            .calibration
            .homography[(0, 0)] *= 1.1;
        assert!(
            trace
                .validate()
                .unwrap_err()
                .to_string()
                .contains("calibrations must match")
        );
        let mut trace = builtin_scenarios().remove(0);
        trace.scene.as_mut().unwrap().screens[0]
            .calibration
            .object_points[0]
            .x += 0.001;
        assert!(
            trace
                .validate()
                .unwrap_err()
                .to_string()
                .contains("calibrations must match")
        );
    }

    #[test]
    fn relabel_uses_edited_grid_and_preserves_selected_target_transactionally() {
        use odyssey_sim::dataset::DatasetError;

        let mut scene = scene_cases()
            .into_iter()
            .find(|(name, _)| name == "nonlinear-projector-grid")
            .unwrap()
            .1;
        let before = scene.frames[0].truth.aimpoint;
        if let Some(surface) = &mut scene.screens[0].calibration.surface {
            for point in &mut surface.positions_m {
                point[0] *= 1.1;
            }
        }
        scene.frames[0].visible_marker_ids = vec!["stale-authoring-label".into()];
        scene.frames[0].truth.aimpoint = [f64::NAN; 2];
        scene.relabel().unwrap();
        scene.validate().unwrap();
        assert!((scene.frames[0].truth.aimpoint[0] - before[0]).abs() > 0.01);
        let pose = &scene.frames[0].camera_to_world;
        let expected = scene.screens[0]
            .geometry()
            .unwrap()
            .raycast(pose.position_m, [0.0, 0.0, 1.0])
            .unwrap()
            .unwrap();
        assert_eq!(scene.frames[0].truth.aimpoint, expected.uv);
        assert_eq!(
            scene.frames[0].truth.position_m,
            Some([pose.position_m[0], -pose.position_m[1], -pose.position_m[2]])
        );
        let labels = scene
            .frames
            .iter()
            .map(|frame| (frame.truth.aimpoint, frame.visible_marker_ids.clone()))
            .collect::<Vec<_>>();
        scene.frames.last_mut().unwrap().camera_to_world.position_m[0] = 99.0;
        assert_eq!(scene.relabel(), Err(DatasetError::TargetMiss));
        for (frame, (aim, visible)) in scene.frames.iter().zip(labels) {
            assert_eq!(frame.truth.aimpoint, aim);
            assert_eq!(frame.visible_marker_ids, visible);
            assert_eq!(frame.truth.screen_id, 0);
        }
    }

    #[test]
    fn relabel_hides_marker_behind_another_display_not_coplanar_endpoints() {
        let mut scene = generate("occlusion-label-test", Trajectory::Stationary([0.32, 0.37]))
            .scene
            .unwrap();
        scene.relabel().unwrap();
        let marker_id = scene.screens[0].marker_ids[0].clone();
        assert!(scene.frames[0].visible_marker_ids.contains(&marker_id));
        let marker = scene.screens[0]
            .geometry()
            .unwrap()
            .marker_world(0)
            .unwrap();
        let origin = scene.frames[0].camera_to_world.position_m;
        let center: [f64; 3] = std::array::from_fn(|i| (origin[i] + marker[i]) * 0.5);
        let mut occluder = scene.screens[0].clone();
        occluder.id = 1;
        occluder.marker_ids = std::array::from_fn(|i| format!("occluder-marker-{i}"));
        for point in &mut occluder.calibration.object_points {
            point.x += 5.0;
        }
        occluder.calibration.surface = Some(odyssey_common::ScreenSurface {
            columns: 2,
            rows: 2,
            positions_m: vec![
                [center[0] - 0.02, center[1] - 0.02, center[2]],
                [center[0] + 0.02, center[1] - 0.02, center[2]],
                [center[0] - 0.02, center[1] + 0.02, center[2]],
                [center[0] + 0.02, center[1] + 0.02, center[2]],
            ],
        });
        scene.screens.push(occluder);
        scene.relabel().unwrap();
        assert!(!scene.frames[0].visible_marker_ids.contains(&marker_id));
        for id in &scene.screens[0].marker_ids[1..] {
            assert!(scene.frames[0].visible_marker_ids.contains(id), "{id}");
        }
    }

    #[test]
    fn geometric_frustum_visibility_honors_radial_distortion() {
        let mut camera = SceneCamera {
            id: "distorted".into(),
            source_id: "source".into(),
            image_size_px: [100, 100],
            intrinsics: [[100.0, 0.0, 50.0], [0.0, 100.0, 50.0], [0.0, 0.0, 1.0]],
            distortion: [0.0; 5],
            sensor: None,
        };
        let pose = ScenePose {
            position_m: [0.0; 3],
            orientation_xyzw: [0.0, 0.0, 0.0, 1.0],
        };
        assert_eq!(
            camera.project_world(&pose, [0.45, 0.0, 1.0]),
            Some([95.0, 50.0])
        );
        camera.distortion[0] = 1.0;
        assert_eq!(camera.project_world(&pose, [0.45, 0.0, 1.0]), None);
    }
}
