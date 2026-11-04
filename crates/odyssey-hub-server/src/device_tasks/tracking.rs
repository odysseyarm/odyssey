use std::time::Duration;

use ahrs::Ahrs as _;
use nalgebra::{
    Isometry3, Matrix3, Matrix3x1, Point2, Point3, Rotation3, Translation3, UnitQuaternion,
    UnitVector3, Vector2, Vector3,
};
use odyssey_common::ros_opencv_intrinsics_type_convert;
use odyssey_cv::foveated::{FoveatedAimpointState, Marker};
use odyssey_cv::{helpers as ats_helpers, to_normalized_image_coordinates, undistort_points};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::{
    AccelerometerEvent, BatteryEvent, DeviceEventKind, ImpactEvent, Pose, TrackingEvent,
};
use odyssey_usb::device::GeneralSettings;
use odyssey_usb::packets::vm::{AccelReport, CombinedMarkersReport, PocMarkersReport};
use opencv_ros_camera::RosOpenCvIntrinsics;
use tracing::{info, trace};

use super::context::{DeviceTaskContext, ScreenCalibrationList};
use super::sensor_streams::DeviceStreamEvent;

/// Time step assumed for the first accelerometer sample, which has no predecessor.
const FIRST_IMU_SAMPLE_DT: Duration = Duration::from_micros(10_000);

/// Madgwick filter sample period before the real one is measured, and its gain.
const MADGWICK_INITIAL_SAMPLE_PERIOD_S: f32 = 1.0 / 100.0;
const MADGWICK_BETA: f32 = 0.1;

/// Optics of one unit: both cameras and the transform between them.
#[derive(Debug)]
pub(super) struct CameraModels {
    near_field: RosOpenCvIntrinsics<f32>,
    wide_field: RosOpenCvIntrinsics<f32>,
    stereo_iso: Isometry3<f32>,
}

impl CameraModels {
    /// Placeholder optics for a unit whose config could not be read (e.g. it starts in
    /// BLE mode). Replaced by [`Self::from_settings`] once an events device attaches.
    fn placeholder() -> Self {
        Self {
            near_field: RosOpenCvIntrinsics::from_params(1.0, 0.0, 1.0, 0.0, 0.0),
            wide_field: RosOpenCvIntrinsics::from_params(1.0, 0.0, 1.0, 0.0, 0.0),
            stereo_iso: Isometry3::identity(),
        }
    }

    pub(super) fn from_settings(settings: &GeneralSettings) -> Self {
        Self {
            near_field: settings.camera_model_nf.clone(),
            wide_field: settings.camera_model_wf.clone(),
            stereo_iso: settings.stereo_iso,
        }
    }
}

/// Turns one unit's sensor reports into tracking events. Owned by the unit's handler task,
/// so the filter state lives exactly as long as the connection does.
pub(super) struct DeviceTracker {
    pub(super) foveated: FoveatedAimpointState,
    prev_accel_timestamp: Option<u32>,
    madgwick: ahrs::Madgwick<f32>,
    orientation: Rotation3<f32>,
    cameras: CameraModels,
}

/// Where the unit points on the screen, and how far away it is.
struct Aim {
    aimpoint: Vector2<f32>,
    distance: f32,
}

/// Outcome of one raycast against the screen calibrations.
struct RaycastSolution {
    pose: Option<Pose>,
    aim: Option<Aim>,
    screen_id: u32,
}

impl DeviceTracker {
    pub(super) fn new(settings: Option<&GeneralSettings>) -> Self {
        Self {
            foveated: FoveatedAimpointState::new(),
            prev_accel_timestamp: None,
            madgwick: ahrs::Madgwick::<f32>::new(MADGWICK_INITIAL_SAMPLE_PERIOD_S, MADGWICK_BETA),
            orientation: Rotation3::identity(),
            cameras: settings.map_or_else(CameraModels::placeholder, CameraModels::from_settings),
        }
    }

    pub(super) fn set_cameras(&mut self, cameras: CameraModels) {
        self.cameras = cameras;
    }

    /// Process one inbound sensor report and publish the resulting events.
    pub(super) async fn handle_event(
        &mut self,
        device: &Device,
        event: DeviceStreamEvent,
        context: &DeviceTaskContext,
    ) {
        trace!(
            "handle_stream_event for device {:02x?}: {}",
            device.uuid,
            event.name()
        );
        match event {
            DeviceStreamEvent::Accel(report) => self.on_accel(device, &report, context).await,
            DeviceStreamEvent::Impact(report) => {
                context.emit_device_event(
                    device,
                    DeviceEventKind::ImpactEvent(ImpactEvent {
                        timestamp: report.timestamp,
                    }),
                );
            }
            DeviceStreamEvent::Battery(report) => {
                info!(
                    "Battery event dispatched for {:02x?}: {}% charging={}",
                    device.uuid, report.percent, report.charging
                );
                context.emit_device_event(
                    device,
                    DeviceEventKind::BatteryEvent(BatteryEvent {
                        percent: report.percent,
                        charging: report.charging,
                    }),
                );
            }
            DeviceStreamEvent::CombinedMarkers(report) => {
                self.on_combined_markers(device, &report, context).await;
            }
            DeviceStreamEvent::PocMarkers(report) => {
                self.on_poc_markers(device, &report, context).await;
            }
        }
    }

    async fn on_accel(
        &mut self,
        device: &Device,
        report: &AccelReport,
        context: &DeviceTaskContext,
    ) {
        let elapsed_us = match self.prev_accel_timestamp {
            Some(previous) => u64::from(report.timestamp.wrapping_sub(previous)),
            None => 0,
        };
        let dt = if elapsed_us == 0 {
            FIRST_IMU_SAMPLE_DT
        } else {
            Duration::from_micros(elapsed_us)
        };
        // Same axis ordering as the vision module (negated xzy).
        let accel = -report.accel.xzy();
        let gyro = -report.gyro.xzy();
        self.foveated.predict(accel, gyro, dt);
        self.prev_accel_timestamp = Some(report.timestamp);

        // The Madgwick filter only exists to give the marker pipeline a gravity vector.
        if elapsed_us != 0 {
            *self.madgwick.sample_period_mut() = elapsed_us as f32 / 1_000_000.0;
        }
        let _ = self.madgwick.update_imu(
            &Vector3::new(report.gyro.x, report.gyro.y, report.gyro.z),
            &Vector3::new(report.accel.x, report.accel.y, report.accel.z),
        );
        self.orientation = self.madgwick.quat.to_rotation_matrix();

        context.emit_device_event(
            device,
            DeviceEventKind::AccelerometerEvent(AccelerometerEvent {
                timestamp: report.timestamp,
                accel: report.accel,
                gyro: report.gyro,
                euler_angles: Vector3::zeros(),
            }),
        );

        // Re-solve the pose from the IMU-predicted state.
        let solution = self.raycast(device, context).await;
        let (Some(pose), Some(aim)) = (solution.pose, solution.aim) else {
            return;
        };
        context.emit_device_event(
            device,
            DeviceEventKind::TrackingEvent(TrackingEvent {
                timestamp: report.timestamp,
                aimpoint: aim.aimpoint,
                pose,
                distance: aim.distance,
                screen_id: solution.screen_id,
            }),
        );
    }

    async fn on_combined_markers(
        &mut self,
        device: &Device,
        report: &CombinedMarkersReport,
        context: &DeviceTaskContext,
    ) {
        let near_points = detected_points(&report.nf_points);
        let wide_points = detected_points(&report.wf_points);
        trace!(
            "CombinedMarkers for {:02x?}: {} NF points, {} WF points",
            device.uuid,
            near_points.len(),
            wide_points.len()
        );
        let near_markers = build_markers_from_points(near_points, &self.cameras.near_field, None);
        let wide_markers = build_markers_from_points(
            wide_points,
            &self.cameras.wide_field,
            Some(&self.cameras.stereo_iso),
        );

        self.observe_markers(&near_markers, &wide_markers, context);
        let solution = self.raycast(device, context).await;
        self.publish_marker_solution(device, solution, context);
    }

    async fn on_poc_markers(
        &mut self,
        device: &Device,
        report: &PocMarkersReport,
        context: &DeviceTaskContext,
    ) {
        let near_markers = build_markers_from_points(
            detected_points(&report.points),
            &self.cameras.near_field,
            None,
        );

        self.observe_markers(&near_markers, &[], context);
        let solution = self.raycast(device, context).await;
        self.publish_marker_solution(device, solution, context);
    }

    fn observe_markers(
        &mut self,
        near_markers: &[Marker],
        wide_markers: &[Marker],
        context: &DeviceTaskContext,
    ) {
        let _observed = self.foveated.observe_markers(
            near_markers,
            wide_markers,
            gravity_in_sensor_frame(&self.orientation),
            &context.screen_calibrations.load(),
        );
    }

    /// Solve pose and aimpoint from the current foveated state, applying the device's stored
    /// zero offset if it has one.
    async fn raycast(&mut self, device: &Device, context: &DeviceTaskContext) -> RaycastSolution {
        let offset = context
            .device_offsets
            .lock()
            .await
            .get(&device.uuid)
            .copied();
        let screen_calibrations = context.screen_calibrations.load();
        let (pose, aimpoint_and_distance) =
            ats_helpers::raycast_update(&screen_calibrations, &mut self.foveated, offset);

        if pose.is_none() {
            trace!(
                "raycast_update returned None pose for device {:02x?}. Screen calibrations count: {}, screen_id: {}",
                device.uuid,
                screen_calibrations.len(),
                self.foveated.screen_id
            );
        }

        RaycastSolution {
            pose: pose
                .map(|(rotation, translation)| transform_pose_for_client(rotation, translation)),
            aim: aimpoint_and_distance.map(|(point, distance)| Aim {
                aimpoint: point.coords,
                distance,
            }),
            screen_id: u32::from(self.foveated.screen_id),
        }
    }

    /// A marker report has no timestamp of its own, and without a pose there is nothing to track.
    fn publish_marker_solution(
        &self,
        device: &Device,
        solution: RaycastSolution,
        context: &DeviceTaskContext,
    ) {
        let Some(pose) = solution.pose else {
            trace!(
                "Raycast failed to produce pose for device {:02x?}, skipping tracking event",
                device.uuid
            );
            return;
        };
        // A pose without an aimpoint still produces an event, with a zero aim.
        let aim = solution.aim.unwrap_or(Aim {
            aimpoint: Vector2::zeros(),
            distance: 0.0,
        });
        trace!(
            "Sending TrackingEvent for device {:02x?}: aimpoint=({:.3}, {:.3}), distance={:.3}",
            device.uuid, aim.aimpoint.x, aim.aimpoint.y, aim.distance
        );
        context.emit_device_event(
            device,
            DeviceEventKind::TrackingEvent(TrackingEvent {
                timestamp: 0,
                aimpoint: aim.aimpoint,
                pose,
                distance: aim.distance,
                screen_id: solution.screen_id,
            }),
        );
    }
}

/// Gravity direction in the frame the marker pipeline expects (y and z swapped).
fn gravity_in_sensor_frame(orientation: &Rotation3<f32>) -> UnitVector3<f32> {
    let global_down = Vector3::z_axis().into_inner();
    let local = orientation.inverse_transform_vector(&global_down);
    UnitVector3::new_normalize(Vector3::new(local.x, local.z, local.y))
}

/// Pixel coordinates of the markers the sensor actually saw; an all-zero point is an empty slot.
fn detected_points(points: &[Point2<u16>]) -> Vec<Point2<f32>> {
    points
        .iter()
        .filter(|p| p.x != 0 || p.y != 0)
        .map(|p| Point2::new(f32::from(p.x), f32::from(p.y)))
        .collect()
}

/// The client's coordinate convention flips y and z relative to the vision pipeline's.
/// The flip is its own inverse, so it both converts to and recovers from client space.
fn client_axis_flip() -> Matrix3<f32> {
    Matrix3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0)
}

fn transform_pose_for_client(rotmat: Matrix3<f32>, transvec: Vector3<f32>) -> Pose {
    let flip = client_axis_flip();
    let rotation = flip * rotmat * flip;
    let translation = flip * Matrix3x1::new(transvec.x, transvec.y, transvec.z);
    Pose {
        rotation,
        translation,
    }
}

/// Zero offset quaternion from a client-side TrackingEvent. Its pose has been through
/// `transform_pose_for_client`, so the flip is undone to recover the raw orientation and
/// position before the offset is computed.
pub(super) fn calculate_zero_offset_from_tracking_event(
    zero_translation: Translation3<f32>,
    zero_target_in_screen_space: Point2<f32>,
    screen_calibrations: &ScreenCalibrationList,
    te: &TrackingEvent,
) -> Option<UnitQuaternion<f32>> {
    let screen_id = u8::try_from(te.screen_id).ok()?;
    if screen_id > odyssey_common::MAX_SCREEN_ID {
        return None;
    }

    let flip = client_axis_flip();
    let raw_rotation = flip * te.pose.rotation * flip;
    let raw_translation = flip * te.pose.translation;

    let orientation = Rotation3::from_matrix_unchecked(raw_rotation);
    let orientation = UnitQuaternion::from_rotation_matrix(&orientation);
    let position: Vector3<f32> =
        Vector3::new(raw_translation.x, raw_translation.y, raw_translation.z);

    let screen_calibration = ats_helpers::get_screen_calibration(screen_calibrations, screen_id)?;
    let zero_target_3d = screen_calibration.point_at_uv(zero_target_in_screen_space)?;
    let iso = Isometry3::from_parts(position.into(), orientation);
    let zero_position = iso * Point3::from(zero_translation.vector);

    let desired_direction = orientation.inverse_transform_vector(&{
        let v = zero_target_3d - zero_position;
        let norm = v.norm();
        if norm.abs() < f32::EPSILON {
            return None;
        }
        v / norm
    });

    let current_forward = Vector3::z();
    UnitQuaternion::rotation_between(&current_forward, &desired_direction)
}

fn build_markers_from_points(
    raw_points: Vec<Point2<f32>>,
    camera_model: &RosOpenCvIntrinsics<f32>,
    stereo_iso: Option<&Isometry3<f32>>,
) -> Vec<Marker> {
    if raw_points.is_empty() {
        return Vec::new();
    }
    let undistorted = undistort_points(camera_model, &raw_points);
    let intrinsics = ros_opencv_intrinsics_type_convert(camera_model);
    undistorted
        .into_iter()
        .map(|pt| {
            let normalized = to_normalized_image_coordinates(pt, &intrinsics, stereo_iso);
            Marker {
                position: Point2::new(normalized.x, normalized.y),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zeroing_aims_at_measured_surface_instead_of_fitted_plane() {
        let mut calibrations = ScreenCalibrationList::new();
        calibrations.push((
            0,
            odyssey_common::ScreenCalibration {
                surface: Some(odyssey_common::ScreenSurface {
                    columns: 2,
                    rows: 2,
                    positions_m: vec![
                        [0.0, 0.0, 0.5],
                        [1.0, 0.0, 0.5],
                        [0.0, 1.0, 0.5],
                        [1.0, 1.0, 0.5],
                    ],
                }),
                rotation: UnitQuaternion::identity(),
                homography: Matrix3::identity(),
                object_points: [Point3::origin(); odyssey_common::MARKER_PATTERN_LEN],
            },
        ));
        let event = TrackingEvent {
            timestamp: 0,
            aimpoint: Vector2::zeros(),
            pose: Pose {
                rotation: Matrix3::identity(),
                translation: Vector3::new(0.0, 0.0, -1.0),
            },
            distance: 1.0,
            screen_id: 0,
        };
        let offset = calculate_zero_offset_from_tracking_event(
            Translation3::identity(),
            Point2::new(0.75, 0.25),
            &calibrations,
            &event,
        )
        .expect("surface target is reachable");
        let expected = Vector3::new(0.75, 0.25, -0.5).normalize();
        assert!((offset * Vector3::z() - expected).norm() < 1e-5);
    }

    #[test]
    fn zeroing_does_not_wrap_a_screen_id_into_another_calibration() {
        let mut calibrations = ScreenCalibrationList::new();
        calibrations.push((
            0,
            odyssey_common::ScreenCalibration {
                surface: None,
                rotation: UnitQuaternion::identity(),
                homography: Matrix3::identity(),
                object_points: [Point3::origin(); odyssey_common::MARKER_PATTERN_LEN],
            },
        ));
        let mut event = TrackingEvent {
            timestamp: 0,
            aimpoint: Vector2::zeros(),
            pose: Pose {
                rotation: Matrix3::identity(),
                translation: Vector3::new(0.0, 0.0, -1.0),
            },
            distance: 1.0,
            screen_id: 0,
        };
        let calculate = |event: &TrackingEvent| {
            calculate_zero_offset_from_tracking_event(
                Translation3::identity(),
                Point2::new(0.5, 0.5),
                &calibrations,
                event,
            )
        };
        assert!(calculate(&event).is_some());
        event.screen_id = 256;
        assert!(calculate(&event).is_none());
    }

    #[test]
    fn level_sensor_sees_gravity_along_y() {
        let gravity = gravity_in_sensor_frame(&Rotation3::identity());
        assert_eq!(gravity.into_inner(), Vector3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn empty_marker_slots_are_dropped() {
        let points = [Point2::new(0, 0), Point2::new(5, 0), Point2::new(0, 7)];
        assert_eq!(
            detected_points(&points),
            vec![Point2::new(5.0, 0.0), Point2::new(0.0, 7.0)]
        );
    }
}
