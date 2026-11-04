use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ahrs::Ahrs as _;
use anyhow::Result;
use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use nalgebra::{
    Isometry3, Matrix3, Matrix3x1, Point2, Point2 as NaPoint2, Point3, Rotation3, Translation3,
    UnitQuaternion, UnitVector3, Vector2, Vector3,
};
use odyssey_common::{ScreenCalibration, ros_opencv_intrinsics_type_convert};
use odyssey_cv::foveated::FoveatedAimpointState;
use odyssey_cv::{helpers as ats_helpers, to_normalized_image_coordinates, undistort_points};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::{
    AccelerometerEvent, DeviceEvent, DeviceEventKind, Event, ImpactEvent, Pose, TrackingEvent,
};
use odyssey_usb::packets::vm::{Packet, PacketData};
use opencv_ros_camera::RosOpenCvIntrinsics;
use tokio::sync::{Mutex, broadcast};
use tracing::{info, trace};

use super::sensor_streams::DeviceStreamEvent;

/// Process a single inbound Packet and emit appropriate events.
/// This function emits the same basic events as prior implementation:
/// - Accelerometer -> AccelerometerEvent
/// - Impact -> ImpactEvent
/// - CombinedMarkers/Poc/Object -> TrackingEvent (basic representation)
/// Additionally, it always emits a PacketEvent containing the raw odyssey_usb vm::Packet
pub(super) async fn handle_stream_event(
    device: &Device,
    event: DeviceStreamEvent,
    event_sender: &broadcast::Sender<Event>,
    device_offsets: &Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
    fv_state: &mut FoveatedAimpointState,
    screen_calibrations: &Arc<
        ArcSwap<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (odyssey_common::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
    prev_accel_timestamp: &mut Option<u32>,
    madgwick: &mut ahrs::Madgwick<f32>,
    orientation: &mut Rotation3<f32>,
    camera_model_nf: &RosOpenCvIntrinsics<f32>,
    camera_model_wf: &RosOpenCvIntrinsics<f32>,
    stereo_iso: &Isometry3<f32>,
) -> Result<()> {
    let event_type = match &event {
        DeviceStreamEvent::Accel(_) => "Accel",
        DeviceStreamEvent::Impact(_) => "Impact",
        DeviceStreamEvent::Battery(_) => "Battery",
        DeviceStreamEvent::CombinedMarkers(_) => "CombinedMarkers",
        DeviceStreamEvent::PocMarkers(_) => "PocMarkers",
        DeviceStreamEvent::Vendor(_, _) => "Vendor",
    };
    trace!(
        "handle_stream_event for device {:02x?}: {}",
        device.uuid, event_type
    );

    match event {
        DeviceStreamEvent::Accel(report) => {
            // Feed IMU into the handler-local foveated state
            let elapsed_us = match prev_accel_timestamp {
                Some(p) => u64::from(report.timestamp.wrapping_sub(*p)),
                None => 0u64,
            };
            let dt = if elapsed_us == 0 {
                Duration::from_micros(10_000) // default small dt if unknown
            } else {
                Duration::from_micros(elapsed_us)
            };
            // Use same axis ordering as vision module (negated xzy)
            let accel = -report.accel.xzy();
            let gyro = -report.gyro.xzy();
            fv_state.predict(accel, gyro, dt);
            *prev_accel_timestamp = Some(report.timestamp);

            // Update Madgwick orientation filter to compute gravity vector later
            if elapsed_us != 0 {
                let sample_period = madgwick.sample_period_mut();
                *sample_period = elapsed_us as f32 / 1_000_000.0;
            }
            let _ = madgwick.update_imu(
                &Vector3::new(report.gyro.x, report.gyro.y, report.gyro.z),
                &Vector3::new(report.accel.x, report.accel.y, report.accel.z),
            );
            *orientation = madgwick.quat.to_rotation_matrix();

            let ev_kind = DeviceEventKind::AccelerometerEvent(AccelerometerEvent {
                timestamp: report.timestamp,
                accel: report.accel,
                gyro: report.gyro,
                euler_angles: Vector3::zeros(),
            });
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(device.clone(), ev_kind)));

            // Attempt to compute an updated tracking solution using the predicted state.
            let sc = screen_calibrations.load();
            let offset_opt = {
                let guard = device_offsets.lock().await;
                guard.get(&device.uuid).copied()
            };
            let (pose_opt, aim_and_d_opt) = ats_helpers::raycast_update(&sc, fv_state, offset_opt);
            if let (Some((rotmat, transvec)), Some((pt, d))) = (pose_opt, aim_and_d_opt) {
                let tracking_pose = transform_pose_for_client(rotmat, transvec);
                let tracking = TrackingEvent {
                    timestamp: report.timestamp,
                    aimpoint: pt.coords,
                    pose: tracking_pose,
                    distance: d,
                    screen_id: u32::from(fv_state.screen_id),
                };
                let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(
                    device.clone(),
                    DeviceEventKind::TrackingEvent(tracking),
                )));
            }
        }
        DeviceStreamEvent::Impact(report) => {
            let ev_kind = DeviceEventKind::ImpactEvent(ImpactEvent {
                timestamp: report.timestamp,
            });
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(device.clone(), ev_kind)));
        }
        DeviceStreamEvent::Battery(report) => {
            info!(
                "Battery event dispatched for {:02x?}: {}% charging={}",
                device.uuid, report.percent, report.charging
            );
            let ev_kind = DeviceEventKind::BatteryEvent(odyssey_hub_common::events::BatteryEvent {
                percent: report.percent,
                charging: report.charging,
            });
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(device.clone(), ev_kind)));
        }
        DeviceStreamEvent::CombinedMarkers(report) => {
            let nf_points_filtered: Vec<_> = report
                .nf_points
                .iter()
                .filter(|p| p.x != 0 || p.y != 0)
                .map(|p| Point2::new(f32::from(p.x), f32::from(p.y)))
                .collect();
            let wf_points_filtered: Vec<_> = report
                .wf_points
                .iter()
                .filter(|p| p.x != 0 || p.y != 0)
                .map(|p| Point2::new(f32::from(p.x), f32::from(p.y)))
                .collect();

            trace!(
                "CombinedMarkers for {:02x?}: {} NF points, {} WF points",
                device.uuid,
                nf_points_filtered.len(),
                wf_points_filtered.len()
            );

            let nf_markers = build_markers_from_points(nf_points_filtered, camera_model_nf, None);
            let wf_markers =
                build_markers_from_points(wf_points_filtered, camera_model_wf, Some(stereo_iso));

            // Observe markers in the handler-local foveated state and attempt raycast/raycast_update.
            let mut tracking_pose: Option<Pose> = None;
            let mut aimpoint = Vector2::new(0.0f32, 0.0f32);
            let mut distance = 0.0f32;

            let screen_id = {
                let gravity_vec = {
                    let global_down = Vector3::z_axis().into_inner();
                    let local = orientation.inverse_transform_vector(&global_down);
                    let local = Vector3::new(local.x, local.z, local.y);
                    UnitVector3::new_normalize(local)
                };

                // Observe markers directly from existing vectors (pass slices, avoid clones)
                let _observed = fv_state.observe_markers(
                    &nf_markers,
                    &wf_markers,
                    gravity_vec,
                    &screen_calibrations.load(),
                );

                // Attempt to compute pose and aimpoint using raycast_update.
                // Pass stored device offset (if any) so it affects the solution.
                let sc = screen_calibrations.load();
                let offset_opt = {
                    let guard = device_offsets.lock().await;
                    guard.get(&device.uuid).copied()
                };
                let (pose_opt, aim_and_d_opt) =
                    ats_helpers::raycast_update(&sc, fv_state, offset_opt);

                if pose_opt.is_none() {
                    trace!(
                        "raycast_update returned None pose for device {:02x?}. Screen calibrations count: {}, fv_state.screen_id: {}",
                        device.uuid,
                        sc.len(),
                        fv_state.screen_id
                    );
                }

                if let Some((rotmat, transvec)) = pose_opt {
                    tracking_pose = Some(transform_pose_for_client(rotmat, transvec));
                }

                if let Some((pt, d)) = aim_and_d_opt {
                    aimpoint = pt.coords;
                    distance = d;
                }

                u32::from(fv_state.screen_id)
            };

            // If raycast_update didn't produce a pose (and thus no aimpoint), short-circuit.
            let tracking_pose = match tracking_pose {
                Some(p) => p,
                None => {
                    trace!(
                        "Raycast failed to produce pose for device {:02x?}, skipping tracking event",
                        device.uuid
                    );
                    return Ok(());
                }
            };

            let tracking = TrackingEvent {
                timestamp: 0,
                aimpoint,
                pose: tracking_pose,
                distance,
                screen_id,
            };
            trace!(
                "Sending TrackingEvent for device {:02x?}: aimpoint=({:.3}, {:.3}), distance={:.3}",
                device.uuid, aimpoint.x, aimpoint.y, distance
            );
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(
                device.clone(),
                DeviceEventKind::TrackingEvent(tracking),
            )));
        }
        DeviceStreamEvent::PocMarkers(report) => {
            let nf_markers = build_markers_from_points(
                report
                    .points
                    .iter()
                    .filter(|p| p.x != 0 || p.y != 0)
                    .map(|p| Point2::new(f32::from(p.x), f32::from(p.y)))
                    .collect(),
                camera_model_nf,
                None,
            );

            let mut tracking_pose: Option<Pose> = None;
            let mut aimpoint = Vector2::new(0.0f32, 0.0f32);
            let mut distance = 0.0f32;

            let screen_id = {
                let gravity_vec = {
                    let global_down = Vector3::z_axis().into_inner();
                    let local = orientation.inverse_transform_vector(&global_down);
                    let local = Vector3::new(local.x, local.z, local.y);
                    UnitVector3::new_normalize(local)
                };
                let wf_empty: Vec<odyssey_cv::foveated::Marker> = Vec::new();
                let _observed = fv_state.observe_markers(
                    &nf_markers,
                    &wf_empty,
                    gravity_vec,
                    &screen_calibrations.load(),
                );

                let sc = screen_calibrations.load();
                let offset_opt = {
                    let guard = device_offsets.lock().await;
                    guard.get(&device.uuid).copied()
                };
                let (pose_opt, aim_and_d_opt) =
                    ats_helpers::raycast_update(&sc, fv_state, offset_opt);

                if let Some((rotmat, transvec)) = pose_opt {
                    tracking_pose = Some(transform_pose_for_client(rotmat, transvec));
                }
                if let Some((pt, d)) = aim_and_d_opt {
                    aimpoint = pt.coords;
                    distance = d;
                }
                u32::from(fv_state.screen_id)
            };

            // If raycast_update didn't produce a pose (and thus no aimpoint), short-circuit.
            let tracking_pose = match tracking_pose {
                Some(p) => p,
                None => return Ok(()),
            };

            let tracking = TrackingEvent {
                timestamp: 0,
                aimpoint,
                pose: tracking_pose,
                distance,
                screen_id,
            };
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(
                device.clone(),
                DeviceEventKind::TrackingEvent(tracking),
            )));
        }
        DeviceStreamEvent::Vendor(tag, vendor) => {
            let vm_pkt = Packet {
                id: 0,
                data: PacketData::Vendor(tag, vendor),
            };
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(
                device.clone(),
                DeviceEventKind::PacketEvent(vm_pkt),
            )));
        }
    }
    Ok(())
}

pub(super) fn transform_pose_for_client(rotmat: Matrix3<f32>, transvec: Vector3<f32>) -> Pose {
    let flip = Matrix3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0);
    let rotation = flip * rotmat * flip;
    let translation = flip * Matrix3x1::new(transvec.x, transvec.y, transvec.z);
    Pose {
        rotation,
        translation,
    }
}

/// Compute zero offset quaternion from a client-side TrackingEvent.
/// The TrackingEvent pose has been through `transform_pose_for_client` (flip applied),
/// so we undo the flip to recover the raw orientation/position before computing the offset.
pub(super) fn calculate_zero_offset_from_tracking_event(
    zero_translation: Translation3<f32>,
    zero_target_in_screen_space: Point2<f32>,
    screen_calibrations: &ArrayVec<
        (u8, ScreenCalibration<f32>),
        { (odyssey_common::MAX_SCREEN_ID + 1) as usize },
    >,
    te: &TrackingEvent,
) -> Option<UnitQuaternion<f32>> {
    let screen_id = te.screen_id as u8;
    if screen_id > odyssey_common::MAX_SCREEN_ID {
        return None;
    }

    // Undo the flip that transform_pose_for_client applied
    let flip = Matrix3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0);
    let raw_rotation = flip * te.pose.rotation * flip;
    let raw_translation = flip * te.pose.translation;

    let orientation = Rotation3::from_matrix_unchecked(raw_rotation);
    let orientation = UnitQuaternion::from_rotation_matrix(&orientation);
    let position: Vector3<f32> =
        Vector3::new(raw_translation.x, raw_translation.y, raw_translation.z);

    let ats_calibrations = screen_calibrations.clone();
    let screen_calibration = ats_helpers::get_screen_calibration(&ats_calibrations, screen_id)?;
    let inv_homography = screen_calibration.homography.try_inverse()?;

    let zero_target_2d = inv_homography.transform_point(&zero_target_in_screen_space);
    let zero_target_3d = Point3::new(zero_target_2d.x, zero_target_2d.y, 0.0);
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

pub(super) fn build_markers_from_points(
    raw_points: Vec<Point2<f32>>,
    camera_model: &RosOpenCvIntrinsics<f32>,
    stereo_iso: Option<&Isometry3<f32>>,
) -> Vec<odyssey_cv::foveated::Marker> {
    if raw_points.is_empty() {
        return Vec::new();
    }
    let undistorted = undistort_points(camera_model, &raw_points);
    let intrinsics = ros_opencv_intrinsics_type_convert(camera_model);
    undistorted
        .into_iter()
        .map(|pt| {
            let normalized = to_normalized_image_coordinates(pt, &intrinsics, stereo_iso);
            odyssey_cv::foveated::Marker {
                position: NaPoint2::new(normalized.x, normalized.y),
            }
        })
        .collect()
}
