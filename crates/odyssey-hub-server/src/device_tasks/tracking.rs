//! Per-unit tracking: turns sensor reports into the events clients see.
//!
//! IMU reports drive the ESKF's prediction and re-solve the pose. Marker reports are matched
//! to the screen's marker pattern by the ESKF (`odyssey_cv::foveated`). Two things hang off a
//! match:
//!
//! - Marker hints. The matched positions go back to lite1 firmware (`marker_hint_sender.rs`) so
//!   its exposure controller measures only real markers. `odyssey_cv::exposure` describes the
//!   whole loop: sensor, firmware exposure controller, `MarkersReport`, ESKF match, hint.
//! - Calibration refinement. Matched frames feed a `CalibrationEstimator`
//!   (`odyssey_cv::dynamic_calibration`) that can improve the near-field camera and the marker
//!   positions for this connection. See the second `impl DeviceTracker` block.
//!
//! Units: each sensor reports on its own grid, and the camera model in the unit's settings is
//! stated on that grid. Pixel-based thresholds need the grid scale, which
//! `MarkersReport::UNITS_PER_PX` (1/64 px, lite1) and `CombinedMarkersReport::UNITS_PER_PX`
//! (PAJ7025) provide. Hint positions are in report units as received, not converted.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ahrs::Ahrs as _;
use arrayvec::ArrayVec;
use nalgebra::{
    Isometry3, Matrix3, Matrix3x1, Point2, Point3, Rotation3, Translation3, UnitQuaternion,
    UnitVector3, Vector2, Vector3,
};
use odyssey_common::ros_opencv_intrinsics_type_convert;
use odyssey_cv::dynamic_calibration::{CalibrationEstimator, Intrinsics, MARKERS, Refinement};
use odyssey_cv::foveated::{FoveatedAimpointState, Marker};
use odyssey_cv::{helpers as ats_helpers, to_normalized_image_coordinates, undistort_points};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::{
    AccelerometerEvent, BatteryEvent, DeviceEventKind, ImpactEvent, Pose, TrackingEvent,
};
use odyssey_usb::device::GeneralSettings;
use odyssey_usb::packets::vm::legacy::CombinedMarkersReport;
use odyssey_usb::packets::vm::{AccelReport, MarkersReport};
use opencv_ros_camera::RosOpenCvIntrinsics;
use tokio::sync::mpsc;
use tracing::{info, trace};

use super::context::{DeviceTaskContext, ScreenCalibrationList};
use super::marker_hint_sender::MarkerHintSender;
use super::messages::DeviceTaskMessage;
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
    /// The near-field camera as configured, restored when the refinement is discarded.
    configured_near_field: RosOpenCvIntrinsics<f32>,
    refinement: NearFieldRefinement,
    hints: MarkerHintSender,
}

/// What this connection has learned about its near-field camera and the marker pattern.
/// Never persisted: it ends with the connection or when the server swaps the calibrations.
#[derive(Debug, Default)]
struct NearFieldRefinement {
    /// The server's list this state was built against, compared by pointer.
    source: Option<Arc<ScreenCalibrationList>>,
    /// `source` with the accepted marker positions. `None` until an estimate is accepted.
    calibrations: Option<Arc<ScreenCalibrationList>>,
    /// Collects frames until it can solve. Cleared after each accepted refinement and replaced
    /// when the tracked screen changes.
    estimator: Option<Box<ScreenEstimator>>,
}

/// A calibration estimator and the screen whose pattern it models.
#[derive(Debug)]
struct ScreenEstimator {
    screen_id: u8,
    estimator: CalibrationEstimator,
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
        let cameras = settings.map_or_else(CameraModels::placeholder, CameraModels::from_settings);
        Self {
            foveated: FoveatedAimpointState::new(),
            prev_accel_timestamp: None,
            madgwick: ahrs::Madgwick::<f32>::new(MADGWICK_INITIAL_SAMPLE_PERIOD_S, MADGWICK_BETA),
            orientation: Rotation3::identity(),
            configured_near_field: cameras.near_field.clone(),
            cameras,
            refinement: NearFieldRefinement::default(),
            hints: MarkerHintSender::default(),
        }
    }

    /// Where marker hints go. Without one the tracker only tracks.
    pub(super) fn set_hint_sink(&mut self, sink: mpsc::WeakSender<DeviceTaskMessage>) {
        self.hints.set_sink(sink);
    }

    pub(super) fn set_cameras(&mut self, cameras: CameraModels) {
        self.configured_near_field = cameras.near_field.clone();
        self.cameras = cameras;
        self.refinement = NearFieldRefinement::default();
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
            DeviceStreamEvent::LegacyCombinedMarkers(report) => {
                self.on_legacy_combined_markers(device, &report, context)
                    .await;
            }
            DeviceStreamEvent::Markers(report) => {
                self.on_markers(device, &report, context).await;
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

    async fn on_legacy_combined_markers(
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
        let wide_markers = build_markers_from_points(
            &wide_points,
            &self.cameras.wide_field,
            Some(&self.cameras.stereo_iso),
        );

        self.observe_markers(
            &near_points,
            &wide_markers,
            CombinedMarkersReport::UNITS_PER_PX,
            context,
        );
        let solution = self.raycast(device, context).await;
        Self::publish_marker_solution(device, solution, context);
    }

    async fn on_markers(
        &mut self,
        device: &Device,
        report: &MarkersReport,
        context: &DeviceTaskContext,
    ) {
        let near_points = detected_points(&report.points);
        let matched = self.observe_markers(&near_points, &[], MarkersReport::UNITS_PER_PX, context);
        // Only a tracking filter yields enough matched points; otherwise the sender holds back
        // and the firmware's hint expires on its own.
        self.hints.send(Instant::now(), &matched);
        let solution = self.raycast(device, context).await;
        Self::publish_marker_solution(device, solution, context);
    }

    /// Feed one near-field frame to the filter, then, if the filter is tracking, give the
    /// matched markers to the calibration estimator. `units_per_px` is the reporting sensor's
    /// grid units per native pixel. Returns the report coordinates of the markers the filter
    /// matched to the pattern.
    fn observe_markers(
        &mut self,
        near_points: &[Point2<f32>],
        wide_markers: &[Marker],
        units_per_px: f64,
        context: &DeviceTaskContext,
    ) -> ArrayVec<[u16; 2], MARKERS> {
        let calibrations = self.calibrations_for_frame(context);
        self.observe_with(near_points, wide_markers, units_per_px, &calibrations)
    }

    /// [`Self::observe_markers`] against calibrations the caller already chose.
    fn observe_with(
        &mut self,
        near_points: &[Point2<f32>],
        wide_markers: &[Marker],
        units_per_px: f64,
        calibrations: &ScreenCalibrationList,
    ) -> ArrayVec<[u16; 2], MARKERS> {
        let near_markers = build_markers_from_points(near_points, &self.cameras.near_field, None);
        let _observed = self.foveated.observe_markers(
            &near_markers,
            wide_markers,
            gravity_in_sensor_frame(&self.orientation),
            calibrations,
        );
        if !self.foveated.init() {
            return ArrayVec::new();
        }
        let screen_id = self.foveated.screen_id;
        let Some(calibration) = ats_helpers::get_screen_calibration(calibrations, screen_id) else {
            return ArrayVec::new();
        };
        let matched = self
            .foveated
            .match_markers_from_eskf(&near_markers, &[], &calibration.object_points)
            .nf;
        self.feed_estimator(
            screen_id,
            &matched,
            &near_markers,
            units_per_px,
            calibrations,
        );
        matched_report_points(&matched, near_points)
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

/// Refinement of the near-field optics and marker pattern, kept per connection.
///
/// The estimator watches frames the filter is tracking. When it accepts a solution,
/// [`Self::apply_refinement`] replaces the tracker's near-field camera and a private copy of the
/// screen calibrations, and later frames are matched with those. The server's list and the
/// unit's stored settings stay untouched. [`Self::calibrations_for_frame`] decides which list a
/// frame uses and when the refinement is discarded.
///
/// `raycast` keeps using the server's list: it reads the screen geometry, and refinement
/// changes only marker positions.
impl DeviceTracker {
    /// The calibrations to track against: the server's, with this connection's refined marker
    /// positions if an estimate was accepted. A new list from the server drops the refinement
    /// and restores the configured camera, since the list describes a pattern that has been
    /// measured again.
    fn calibrations_for_frame(
        &mut self,
        context: &DeviceTaskContext,
    ) -> Arc<ScreenCalibrationList> {
        let loaded = context.screen_calibrations.load_full();
        let current = self
            .refinement
            .source
            .as_ref()
            .is_some_and(|source| Arc::ptr_eq(source, &loaded));
        if !current {
            self.cameras.near_field = self.configured_near_field.clone();
            self.refinement = NearFieldRefinement {
                source: Some(Arc::clone(&loaded)),
                ..NearFieldRefinement::default()
            };
        }
        self.refinement.calibrations.clone().unwrap_or(loaded)
    }

    /// Gives the estimator for `screen_id` the frame's matched markers, and applies a solution
    /// once it accepts one. `matched` indexes `near_markers` in pattern order.
    fn feed_estimator(
        &mut self,
        screen_id: u8,
        matched: &[Option<usize>; MARKERS],
        near_markers: &[Marker],
        units_per_px: f64,
        calibrations: &ScreenCalibrationList,
    ) {
        let near_field = &self.cameras.near_field;
        let (fx, fy) = (near_field.p.m11, near_field.p.m22);
        let (cx, cy) = (near_field.p.m13, near_field.p.m23);
        if self
            .refinement
            .estimator
            .as_ref()
            .is_none_or(|current| current.screen_id != screen_id)
        {
            let Some(calibration) = ats_helpers::get_screen_calibration(calibrations, screen_id)
            else {
                return;
            };
            // The camera model is stated in report units, so the estimator's intrinsics and
            // the observations below are too; `units_per_px` scales its pixel thresholds.
            let base = Intrinsics {
                fx: f64::from(fx),
                fy: f64::from(fy),
                cx: f64::from(cx),
                cy: f64::from(cy),
            };
            let markers = calibration.object_points.map(nalgebra::OPoint::cast);
            self.refinement.estimator = Some(Box::new(ScreenEstimator {
                screen_id,
                estimator: CalibrationEstimator::new(base, markers, units_per_px),
            }));
        }
        let Some(ScreenEstimator { estimator, .. }) = self.refinement.estimator.as_deref_mut()
        else {
            return;
        };
        // The marker positions are normalized by the same projection matrix, so this restores
        // the pixel the unit reported (after undistortion).
        let observations = matched.map(|index| {
            index.map(|i| {
                let n = near_markers[i].position;
                Vector2::new(f64::from(fx * n.x + cx), f64::from(fy * n.y + cy))
            })
        });
        let pose = Isometry3::from_parts(
            Translation3::from(self.foveated.filter.position.coords.cast::<f64>()),
            self.foveated.filter.orientation.cast::<f64>(),
        );
        estimator.observe(&pose, observations);
        if !estimator.ready() {
            return;
        }
        if let Some(refinement) = estimator.solve() {
            self.apply_refinement(screen_id, &refinement, calibrations);
        }
    }

    /// In memory only: the unit's stored settings and the server's calibrations stay as they
    /// were, so a reconnect starts from the configured values again.
    fn apply_refinement(
        &mut self,
        screen_id: u8,
        refinement: &Refinement,
        calibrations: &ScreenCalibrationList,
    ) {
        let Some(near_field) = refined_near_field(&self.cameras.near_field, &refinement.intrinsics)
        else {
            return;
        };
        let mut refined = calibrations.clone();
        for (id, calibration) in &mut refined {
            if *id == screen_id {
                calibration.object_points = refinement.markers.map(nalgebra::OPoint::cast);
            }
        }
        info!(
            "Refined near-field optics for screen {screen_id}: focal x{:.4} (sd {:.4}), held-out RMS {:.2} -> {:.2}",
            refinement.intrinsics.fx / f64::from(self.cameras.near_field.p.m11),
            refinement.focal_sd,
            refinement.rms_before,
            refinement.rms_after
        );
        self.cameras.near_field = near_field;
        self.refinement.calibrations = Some(Arc::new(refined));
        self.refinement.estimator = None;
    }
}

/// Gravity direction in the frame the marker pipeline expects (y and z swapped).
fn gravity_in_sensor_frame(orientation: &Rotation3<f32>) -> UnitVector3<f32> {
    let global_down = Vector3::z_axis().into_inner();
    let local = orientation.inverse_transform_vector(&global_down);
    UnitVector3::new_normalize(Vector3::new(local.x, local.z, local.y))
}

/// Pixel coordinates of the markers the sensor actually saw. An all-zero point is an empty slot.
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

/// Zero offset quaternion from a client-side `TrackingEvent`. Its pose has been through
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
    raw_points: &[Point2<f32>],
    camera_model: &RosOpenCvIntrinsics<f32>,
    stereo_iso: Option<&Isometry3<f32>>,
) -> Vec<Marker> {
    if raw_points.is_empty() {
        return Vec::new();
    }
    let undistorted = undistort_points(camera_model, raw_points);
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

/// Report coordinates of the markers the filter matched, in pattern order.
fn matched_report_points(
    matched: &[Option<usize>; MARKERS],
    raw_points: &[Point2<f32>],
) -> ArrayVec<[u16; 2], MARKERS> {
    matched
        .iter()
        .flatten()
        .filter_map(|&index| raw_points.get(index))
        .map(|p| [p.x.round() as u16, p.y.round() as u16])
        .collect()
}

/// `current` with the focal length and principal point of `refined`. The pipeline normalizes
/// by `P` after undistorting with `K`, so only `K` changes the result; both move together
/// (`K` by the shift in its own pixel scale) to stay consistent.
fn refined_near_field(
    current: &RosOpenCvIntrinsics<f32>,
    refined: &Intrinsics,
) -> Option<RosOpenCvIntrinsics<f32>> {
    let scale_x = (refined.fx / f64::from(current.p.m11)) as f32;
    let scale_y = (refined.fy / f64::from(current.p.m22)) as f32;
    let shift_x = (refined.cx - f64::from(current.p.m13)) as f32;
    let shift_y = (refined.cy - f64::from(current.p.m23)) as f32;
    let (mut p, mut k) = (current.p, current.k);
    k.m13 += shift_x * current.k.m11 / current.p.m11;
    k.m23 += shift_y * current.k.m22 / current.p.m22;
    p.m13 += shift_x;
    p.m23 += shift_y;
    p.m11 *= scale_x;
    p.m22 *= scale_y;
    k.m11 *= scale_x;
    k.m22 *= scale_y;
    RosOpenCvIntrinsics::from_components(p, k, current.distortion.clone(), current.rect).ok()
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

    /// The calibration wizard's edge pattern on a 55 inch screen (see `tests/marker_order.rs`).
    fn edge_calibrations() -> Arc<ScreenCalibrationList> {
        let mut calibrations = ScreenCalibrationList::new();
        calibrations.push((
            0,
            odyssey_common::ScreenCalibration {
                surface: None,
                rotation: UnitQuaternion::identity(),
                homography: Matrix3::identity(),
                object_points: [
                    [0.24, 0.68],
                    [0.24, 0.0],
                    [0.96, 0.0],
                    [0.6, 0.0],
                    [0.6, 0.68],
                    [0.96, 0.68],
                ]
                .map(|[x, y]| Point3::new(x, y, 0.0)),
            },
        ));
        Arc::new(calibrations)
    }

    fn lite1_tracker() -> DeviceTracker {
        let mut tracker = DeviceTracker::new(None);
        tracker.set_cameras(CameraModels {
            near_field: RosOpenCvIntrinsics::from_params(12645.0, 0.0, 12645.0, 10240.0, 7680.0),
            ..CameraModels::placeholder()
        });
        tracker
    }

    /// Report coordinates of the pattern as a level camera 2.5 m in front of the screen sees it,
    /// listed out of pattern order.
    fn seen_from_front(calibrations: &ScreenCalibrationList) -> Vec<Point2<f32>> {
        let camera_at = Vector3::new(0.6, 0.612, -2.5);
        [3, 0, 5, 1, 4, 2]
            .map(|i| {
                let at = calibrations[0].1.object_points[i].coords - camera_at;
                Point2::new(
                    12645.0 * at.x / at.z + 10240.0,
                    12645.0 * at.y / at.z + 7680.0,
                )
            })
            .to_vec()
    }

    #[test]
    fn a_tracking_filter_reports_the_pattern_points_it_matched() {
        let calibrations = edge_calibrations();
        let seen = seen_from_front(&calibrations);
        let mut tracker = lite1_tracker();
        let matched = tracker.observe_with(&seen, &[], MarkersReport::UNITS_PER_PX, &calibrations);
        assert!(tracker.foveated.init());
        let mut expected: Vec<[u16; 2]> = seen
            .iter()
            .map(|p| [p.x.round() as u16, p.y.round() as u16])
            .collect();
        let mut got = matched.to_vec();
        expected.sort_unstable();
        got.sort_unstable();
        assert_eq!(got, expected);
    }

    #[test]
    fn hints_wait_for_an_initialized_filter() {
        let calibrations = edge_calibrations();
        let seen = seen_from_front(&calibrations);
        let (tx, mut rx) = mpsc::channel(8);
        let mut tracker = lite1_tracker();
        tracker.set_hint_sink(tx.downgrade());

        // Three of the six points cannot start the filter, so nothing is matched or sent.
        let matched =
            tracker.observe_with(&seen[..3], &[], MarkersReport::UNITS_PER_PX, &calibrations);
        assert!(!tracker.foveated.init());
        assert!(matched.is_empty());
        assert!(!tracker.hints.send(Instant::now(), &matched));
        rx.try_recv().unwrap_err();

        let matched = tracker.observe_with(&seen, &[], MarkersReport::UNITS_PER_PX, &calibrations);
        assert!(tracker.hints.send(Instant::now(), &matched));
        assert!(matches!(
            rx.try_recv(),
            Ok(DeviceTaskMessage::MarkerHint(_))
        ));
    }

    #[test]
    fn matched_points_skip_empty_slots_and_unknown_indices() {
        let raw = [Point2::new(10.4, 20.6), Point2::new(30.0, 40.0)];
        let matched = [Some(1), None, Some(0), Some(9), None, None];
        assert_eq!(
            matched_report_points(&matched, &raw).as_slice(),
            &[[30, 40], [10, 21]]
        );
    }

    #[test]
    fn refined_optics_move_the_focal_length_and_principal_point_in_p_and_k() {
        let current = RosOpenCvIntrinsics::from_params(12645.0_f32, 0.0, 12645.0, 10240.0, 7680.0);
        let refined = refined_near_field(
            &current,
            &Intrinsics {
                fx: 12898.0,
                fy: 12898.0,
                cx: 10250.0,
                cy: 7670.0,
            },
        )
        .expect("invertible");
        for m in [refined.p.fixed_view::<3, 3>(0, 0).into_owned(), refined.k] {
            assert!((m.m11 - 12898.0).abs() < 0.1 && (m.m22 - 12898.0).abs() < 0.1);
            assert!((m.m13 - 10250.0).abs() < 0.1 && (m.m23 - 7670.0).abs() < 0.1);
        }
    }
}
