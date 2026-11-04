use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use ahrs::Ahrs as _;
use anyhow::{Context as _, Result, bail};
use arrayvec::ArrayVec;
use nalgebra::{Isometry3, Matrix3, Point2, Rotation3, Translation3, UnitVector3, Vector3};
use odyssey_cv::{calculate_rotational_offset, to_normalized_image_coordinates};
use odyssey_protocol::PocMarkersReport;
use odyssey_usb::device::{GeneralSettings, VmDevice};
use odyssey_usb::packets::vm::{CombinedMarkersReport, MotData, PacketData};
use opencv_ros_camera::RosOpenCvIntrinsics;
use parking_lot::Mutex;
use tokio_stream::StreamExt as _;

use crate::{Marker, TestFrame};

/// Most markers either sensor reports in one frame.
const MAX_MARKERS: usize = 16;

/// The tracking state shared between the UI and the device-stream tasks.
///
/// Lock order is the runner first, then `packets` or `datapoints`.
pub struct MotRunner {
    pub state: crate::MotState,
    pub device: Option<VmDevice>,
    pub general_config: GeneralSettings,
    pub record_impact: bool,
    pub record_packets: bool,
    pub datapoints: Arc<Mutex<Vec<TestFrame>>>,
    pub packets: Arc<Mutex<Vec<(u128, PacketData)>>>,
    pub wfnf_realign: bool,
    pub screen_calibrations: ArrayVec<
        (u8, odyssey_common::ScreenCalibration<f32>),
        { (odyssey_common::MAX_SCREEN_ID + 1) as usize },
    >,
}

impl std::fmt::Debug for MotRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MotRunner")
            .field("record_impact", &self.record_impact)
            .field("record_packets", &self.record_packets)
            .field("wfnf_realign", &self.wfnf_realign)
            .finish_non_exhaustive()
    }
}

impl MotRunner {
    /// Appends `packet_data` to the recording, stamped with the current time.
    fn push_recorded_packet(&self, packet_data: PacketData) -> Result<()> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("cannot record packets before the Unix epoch")?
            .as_millis();
        self.packets.lock().push((timestamp, packet_data));
        Ok(())
    }
}

/// Starts one acquisition session for the currently selected device.
///
/// Ending any stream ends the session and drops the other subscriptions. The
/// caller owns cancellation and must await it before starting a replacement.
pub async fn run(runner: Arc<Mutex<MotRunner>>) -> Result<()> {
    let device = runner.lock().device.clone().context("no device selected")?;
    tokio::select! {
        result = markers_loop(&runner, &device) => result,
        result = accel_stream(&runner, &device) => result,
        result = impact_loop(&runner, &device) => result,
    }
}

pub async fn frame_loop(runner: Arc<Mutex<MotRunner>>) -> Result<()> {
    let device = runner.lock().device.clone().context("no device selected")?;
    let mut mot_data_stream = device
        .stream_mot_data()
        .await
        .context("failed to start object report stream")?;
    while let Some(mot_data) = mot_data_stream.next().await {
        let nf_data = ArrayVec::<MotData, MAX_MARKERS>::from_iter(mot_data.mot_data_nf);
        let wf_data = ArrayVec::<MotData, MAX_MARKERS>::from_iter(mot_data.mot_data_wf);

        let mut runner = runner.lock();
        let state = &mut runner.state;
        state.nf_data = Some(nf_data);
        state.wf_data = Some(wf_data);

        if runner.record_packets {
            runner.push_recorded_packet(PacketData::ObjectReport(mot_data))?;
        }
    }
    bail!("object report stream ended")
}

fn update_raycast(runner: &mut MotRunner) {
    let screen_calibrations = &runner.screen_calibrations;
    let fv_state = &mut runner.state.fv_state;
    let offset = runner.state.fv_zero_offset;
    let (pose, aimpoint_and_d) =
        odyssey_cv::helpers::raycast_update(screen_calibrations, fv_state, Some(offset));
    if let Some(pose) = pose {
        let flip_yz = Matrix3::from_diagonal(&Vector3::new(1., -1., -1.));

        let rot = Rotation3::from_matrix_unchecked(flip_yz * pose.0 * flip_yz);
        let trans = Translation3::from(flip_yz * pose.1);
        runner.state.rotation_mat = *rot.matrix();
        runner.state.translation_mat = trans.vector;
    }
    if let Some(aimpoint_and_d) = aimpoint_and_d {
        runner.state.fv_aimpoint = aimpoint_and_d.0;
        runner.state.distance = aimpoint_and_d.1;
    }
}

/// Wrapper to track whether markers came from POC or combined report
enum MarkersReport {
    Combined(CombinedMarkersReport),
    Poc(PocMarkersReport),
}

/// One camera's detected points, after dropping empty slots, undistorting
/// and normalizing them.
struct ProcessedPoints {
    /// `(mot_id, pixel position)` of every point that was detected.
    tuples: ArrayVec<(u8, Point2<f32>), MAX_MARKERS>,
    normalized: ArrayVec<Point2<f32>, MAX_MARKERS>,
    markers: ArrayVec<Marker, MAX_MARKERS>,
}

/// Applies the camera model to a camera's raw points.
fn process_points(
    points: &[Point2<u16>],
    camera_model: &RosOpenCvIntrinsics<f32>,
    stereo_iso: Option<&Isometry3<f32>>,
) -> ProcessedPoints {
    let tuples = create_point_tuples(points);
    let points_raw: ArrayVec<_, MAX_MARKERS> = tuples.iter().map(|&(_, p)| p).collect();
    let intrinsics = odyssey_common::ros_opencv_intrinsics_type_convert(camera_model);
    let points_transformed = odyssey_cv::undistort_points(&intrinsics, &points_raw);
    let normalized: ArrayVec<_, MAX_MARKERS> = points_transformed
        .iter()
        .map(|&p| to_normalized_image_coordinates(p, &intrinsics, stereo_iso))
        .collect();
    let markers = tuples
        .iter()
        .zip(&normalized)
        .map(|(&(mot_id, _), &normalized)| Marker {
            mot_id,
            pattern_id: None,
            normalized,
        })
        .collect();
    ProcessedPoints {
        tuples,
        normalized,
        markers,
    }
}

/// Re-derives the rotation between the wide- and near-field cameras from a
/// frame in which both see the same markers.
fn realign_stereo_rotation(
    runner: &mut MotRunner,
    wf_normalized: &[Point2<f32>],
    nf_normalized: &[Point2<f32>],
    gravity_vec: UnitVector3<f32>,
) {
    let Some((wf_match_ix, _, _)) = odyssey_cv::foveated::identify_markers(
        wf_normalized,
        gravity_vec.cast(),
        &runner.screen_calibrations,
    ) else {
        return;
    };
    let wf_match = wf_match_ix.map(|i| wf_normalized[i].coords);
    let (nf_match_ix, _) = odyssey_cv::foveated::match3(nf_normalized, &wf_match);
    if !nf_match_ix.iter().all(Option::is_some) {
        return;
    }
    let nf_ordered = nf_match_ix.map(|i| {
        let i = i.expect("every nf marker matched: checked above");
        nf_normalized[i].coords.push(1.0)
    });
    let wf_ordered = wf_match_ix.map(|i| wf_normalized[i].coords.push(1.0));
    let q = calculate_rotational_offset(&wf_ordered, &nf_ordered);
    runner.general_config.stereo_iso.rotation *= q.cast();
    runner.wfnf_realign = false;
}

async fn markers_loop(runner: &Mutex<MotRunner>, device: &VmDevice) -> Result<()> {
    let mut combined_markers_stream = device
        .stream_combined_markers()
        .await
        .context("failed to start combined markers stream")?;
    let mut poc_markers_stream = device
        .stream_poc_markers()
        .await
        .context("failed to start POC markers stream")?;

    loop {
        // Either subscription ending is a session failure, even while the
        // other remains alive. Merging streams would hide that partial loss.
        let report = tokio::select! {
            report = combined_markers_stream.next() => {
                MarkersReport::Combined(report.context("combined markers stream ended")?)
            },
            report = poc_markers_stream.next() => {
                MarkersReport::Poc(report.context("POC markers stream ended")?)
            },
        };
        let (is_poc, nf_points, wf_points) = match report {
            MarkersReport::Poc(poc) => (true, poc.points, Default::default()),
            MarkersReport::Combined(combined) => (false, combined.nf_points, combined.wf_points),
        };

        let mut runner = runner.lock();

        // Track whether this is a POC marker report
        runner.state.is_poc_markers = is_poc;

        let nf = process_points(&nf_points, &runner.general_config.camera_model_nf, None);
        let wf = process_points(
            &wf_points,
            &runner.general_config.camera_model_wf,
            Some(&runner.general_config.stereo_iso.cast()),
        );

        runner.state.nf_markers2 = nf.markers;
        runner.state.wf_markers2 = wf.markers;

        let gravity_vec = UnitVector3::new_unchecked(
            runner
                .state
                .orientation
                .inverse_transform_vector(&Vector3::z_axis())
                .xzy(),
        );

        if runner.wfnf_realign {
            realign_stereo_rotation(&mut runner, &wf.normalized, &nf.normalized, gravity_vec);
        }

        // Observe markers
        let nf_markers_cv = runner
            .state
            .nf_markers2
            .iter()
            .map(Marker::odyssey_cv_marker)
            .collect::<ArrayVec<_, MAX_MARKERS>>();
        let wf_markers_cv = runner
            .state
            .wf_markers2
            .iter()
            .map(Marker::odyssey_cv_marker)
            .collect::<ArrayVec<_, MAX_MARKERS>>();
        let screen_calibrations = runner.screen_calibrations.clone();
        runner.state.fv_state.observe_markers(
            &nf_markers_cv,
            &wf_markers_cv,
            gravity_vec.cast(),
            &screen_calibrations,
        );

        update_raycast(&mut runner);

        // Update runner state. Nothing identifies WF markers into the
        // `*_markers` / `wf_reproj` lists any more, so every point stays
        // unmatched and those lists stay empty; the canvases still read them.
        runner.state.nf_points = nf.tuples;
        runner.state.wf_points = wf.tuples;
        runner.state.nf_markers.clear();
        runner.state.wf_markers.clear();
        runner.state.wf_reproj.clear();

        // Update aimpoint history
        let gravity_angle = -gravity_vec.z.atan2(-gravity_vec.x).to_degrees() + 90.0;
        runner.state.push_aimpoint(gravity_angle);

        // Record packets if enabled
        if runner.record_packets {
            let packet_data = if is_poc {
                PacketData::PocMarkersReport(PocMarkersReport { points: nf_points })
            } else {
                PacketData::CombinedMarkersReport(CombinedMarkersReport {
                    nf_points,
                    wf_points,
                })
            };
            runner.push_recorded_packet(packet_data)?;
        }
    }
}

fn create_point_tuples(points: &[Point2<u16>]) -> ArrayVec<(u8, Point2<f32>), MAX_MARKERS> {
    points
        .iter()
        .enumerate()
        .filter(|(_id, pos)| **pos != Point2::new(0, 0))
        .map(|(id, pos)| (id as u8, Point2::new(f32::from(pos.x), f32::from(pos.y))))
        .collect()
}

async fn accel_stream(runner: &Mutex<MotRunner>, device: &VmDevice) -> Result<()> {
    let mut accel_stream = device
        .stream_accel()
        .await
        .context("failed to start accelerometer stream")?;
    let mut prev_timestamp = None;
    while let Some(accel) = accel_stream.next().await {
        let mut runner = runner.lock();
        // Keep the raw report: playback applies the saved calibration itself.
        if runner.record_packets {
            runner.push_recorded_packet(PacketData::AccelReport(accel))?;
        }
        let accel_odr = runner.general_config.accel_config.accel_odr;
        anyhow::ensure!(accel_odr != 0, "accelerometer sample rate must be nonzero");

        // correct accel and gyro bias and scale
        let accel = odyssey_usb::packets::vm::AccelReport {
            accel: accel.corrected_accel(&runner.general_config.accel_config),
            gyro: accel.corrected_gyro(&runner.general_config.gyro_config),
            timestamp: accel.timestamp,
        };

        // The device clock went backwards (e.g. it restarted or wrapped).
        // Use the nominal period for the first sample of the new clock.
        if prev_timestamp.is_some_and(|prev| accel.timestamp < prev) {
            prev_timestamp = None;
        }

        let period = match prev_timestamp {
            Some(previous) => Duration::from_micros(u64::from(accel.timestamp - previous)),
            None => Duration::from_secs_f32(1. / f32::from(accel_odr)),
        };
        prev_timestamp = Some(accel.timestamp);
        if period.is_zero() {
            continue;
        }
        runner
            .state
            .fv_state
            .predict(-accel.accel.xzy(), -accel.gyro.xzy(), period);
        *runner.state.madgwick.sample_period_mut() = period.as_secs_f32();

        let _ = runner
            .state
            .madgwick
            .update_imu(&Vector3::from(accel.gyro), &Vector3::from(accel.accel));
        runner.state.orientation = runner.state.madgwick.quat.to_rotation_matrix();

        odyssey_cv::series_add!(
            imu_data,
            (-accel.accel.xzy().cast(), -accel.gyro.xzy().cast())
        );

        update_raycast(&mut runner);
    }
    bail!("accelerometer stream ended")
}

async fn impact_loop(runner: &Mutex<MotRunner>, device: &VmDevice) -> Result<()> {
    let mut impact_stream = device
        .stream_impact()
        .await
        .context("failed to start impact stream")?;
    while let Some(impact) = impact_stream.next().await {
        let runner = runner.lock();
        if runner.record_impact {
            let (aimpoint, cant, position) = runner.state.latest_aimpoint();
            runner.datapoints.lock().push(TestFrame {
                fv_aimpoint_x: Some(aimpoint.x),
                fv_aimpoint_y: Some(aimpoint.y),
                opposite_cant: Some(cant),
                position_x: Some(position.x),
                position_y: Some(position.y),
                position_z: Some(position.z),
            });
        }
        if runner.record_packets {
            runner.push_recorded_packet(PacketData::ImpactReport(impact))?;
        }
    }
    bail!("impact stream ended")
}
