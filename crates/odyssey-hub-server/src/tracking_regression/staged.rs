//! Raw sensor reports projected from a staged scene's own frames, so the production
//! tracker can be replayed against any scene authored outside the built-in cases.

use nalgebra::{Quaternion, UnitQuaternion, Vector3};
use odyssey_sim::dataset::{SceneDescription, ScenePose};
use odyssey_usb::device::GeneralSettings;
use opencv_ros_camera::RosOpenCvIntrinsics;

use super::{
    Error, Input, Provenance, SCHEMA_VERSION, Sample, Screen, Source, Thresholds, Trace, require,
};

/// Tracker convergence time: earlier samples run but are not scored.
const WARMUP_US: u64 = 200_000;
const SENSOR_CLOCK_ORIGIN: u32 = 1_000;
const STAGED_DEVICE_UUID: [u8; 6] = [0x53, 0x59, 0x4e, 0, 0, 2];
const GRAVITY_M_S2: f64 = 9.81;

fn orientation(pose: &ScenePose) -> UnitQuaternion<f64> {
    let [x, y, z, w] = pose.orientation_xyzw;
    UnitQuaternion::new_normalize(Quaternion::new(w, x, y, z))
}

/// Replay input for a scene with one screen and one camera.
///
/// With `tracker_knows_surface` false the tracker's calibration is the planar
/// fit alone while the scene keeps the measured surface, which shows what a
/// real display's waviness costs an uncalibrated tracker. IMU gyro is zero, as
/// in the built-in cases; acceleration comes from the frames' positions.
pub fn trace_from_scene(
    name: &str,
    scene: SceneDescription,
    tracker_knows_surface: bool,
) -> Result<Trace, Error> {
    require(
        scene.screens.len() == 1 && scene.cameras.len() == 1,
        "staged replay needs exactly one screen and one camera",
    )?;
    scene
        .validate()
        .map_err(|error| Error::Invalid(error.to_string()))?;
    let screen = &scene.screens[0];
    let camera = &scene.cameras[0];
    let geometry = screen
        .geometry()
        .map_err(|error| Error::Invalid(error.to_string()))?;
    let markers: Vec<[f64; 3]> = (0..screen.marker_ids.len())
        .map(|index| {
            geometry
                .marker_world(index)
                .ok_or_else(|| Error::Invalid("screen marker missing".to_owned()))
        })
        .collect::<Result<_, _>>()?;
    let [[fx, skew, cx], [_, fy, cy], _] = camera.intrinsics;
    let optics =
        RosOpenCvIntrinsics::from_params(fx as f32, skew as f32, fy as f32, cx as f32, cy as f32);
    let settings = GeneralSettings {
        camera_model_nf: optics.clone(),
        camera_model_wf: optics,
        ..GeneralSettings::default()
    };
    let mut samples = Vec::with_capacity(scene.frames.len() * 2);
    for (index, frame) in scene.frames.iter().enumerate() {
        let acceleration = if index == 0 || index + 1 == scene.frames.len() {
            Vector3::zeros()
        } else {
            let (before, here, after) = (
                scene.frames[index - 1].camera_to_world.position_m,
                frame.camera_to_world.position_m,
                scene.frames[index + 1].camera_to_world.position_m,
            );
            let dt = (scene.frames[index + 1].elapsed_us - scene.frames[index - 1].elapsed_us)
                as f64
                / 2_000_000.0;
            if dt > 0.0 {
                (Vector3::from(after) - Vector3::from(here) * 2.0 + Vector3::from(before))
                    / (dt * dt)
            } else {
                Vector3::zeros()
            }
        };
        // Wire axes are the negated x, z, y of camera coordinates; the sensor reads
        // proper acceleration, so gravity is subtracted before rotating in.
        let proper = orientation(&frame.camera_to_world)
            .inverse_transform_vector(&(acceleration - Vector3::new(0.0, GRAVITY_M_S2, 0.0)));
        let truth = frame.truth.clone();
        let score = frame.elapsed_us >= WARMUP_US;
        samples.push(Sample {
            elapsed_us: frame.elapsed_us,
            source_id: camera.source_id.clone(),
            input: Input::Imu {
                timestamp: SENSOR_CLOCK_ORIGIN.wrapping_add(frame.elapsed_us as u32),
                accel: [-proper.x as f32, -proper.z as f32, -proper.y as f32],
                gyro: [0.0; 3],
            },
            truth: Some(truth.clone()),
            expectation: None,
            score,
            image_artifacts: Vec::new(),
        });
        let mut points = [[0; 2]; 16];
        for (output, (marker_id, marker)) in points
            .iter_mut()
            .zip(screen.marker_ids.iter().zip(&markers))
        {
            if !frame.visible_marker_ids.contains(marker_id) {
                continue;
            }
            if let Some([u, v]) = camera.project_world(&frame.camera_to_world, *marker)
                && (1.0..=f64::from(camera.image_size_px[0] - 1)).contains(&u)
                && (1.0..=f64::from(camera.image_size_px[1] - 1)).contains(&v)
            {
                *output = [u.round() as u16, v.round() as u16];
            }
        }
        samples.push(Sample {
            elapsed_us: frame.elapsed_us,
            source_id: camera.source_id.clone(),
            input: Input::Markers { points },
            truth: Some(truth),
            expectation: None,
            score,
            image_artifacts: Vec::new(),
        });
    }
    let mut tracker_calibration = screen.calibration.clone();
    if !tracker_knows_surface {
        tracker_calibration.surface = None;
    }
    Ok(Trace {
        schema_version: SCHEMA_VERSION,
        name: name.to_owned(),
        provenance: Provenance {
            description: "Raw observations projected from a staged scene; truth is the scene's own geometry, not a physical capture.".to_owned(),
            generator: Some("odyssey-stage".to_owned()),
        },
        sources: vec![Source {
            id: camera.source_id.clone(),
            device_uuid: STAGED_DEVICE_UUID,
            clock_basis: "Synthetic arrival microseconds from the scene's first frame; sensor IMU microseconds are the same clock plus 1000, modulo 2^32; equal-time array order is IMU then optics.".to_owned(),
            settings,
        }],
        screens: vec![Screen {
            id: screen.id,
            calibration: tracker_calibration,
        }],
        samples,
        thresholds: Thresholds {
            min_availability: 0.9,
            min_truth_matches: 1,
            max_aim_rmse: Some(0.02),
            max_position_rmse_m: Some(0.04),
            max_jump: Some(0.03),
            max_reacquisition_us: None,
        },
        scene: Some(scene),
    })
}
