//! Deterministic production-tracker replay and shared capture oracle. Arrival time is not sensor time.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;

use arc_swap::ArcSwap;
use nalgebra::{Point2, Vector3};
use odyssey_common::ScreenCalibration;
use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event, TrackingEvent};
use odyssey_sim::dataset::SceneDescription;
pub use odyssey_sim::dataset::SceneTruth as Truth;
use odyssey_usb::device::GeneralSettings;
use odyssey_usb::packets::vm::{AccelReport, CombinedMarkersReport, PocMarkersReport};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

use super::context::{DeviceTaskContext, ScreenCalibrationList};
use super::sensor_streams::DeviceStreamEvent;
use super::tracking::DeviceTracker;

pub mod marker_noise;
pub mod recording;
pub mod scenarios;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SAMPLES: usize = 100_000;
const MAX_SOURCES: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("tracking regression: {0}")]
    Invalid(String),
    #[error("tracking regression JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("tracking regression I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("tracking regression CBOR: {0}")]
    CborDecode(#[from] minicbor_serde::error::DecodeError),
    #[error("tracking regression CBOR: {0}")]
    CborEncode(#[from] minicbor_serde::error::EncodeError<core::convert::Infallible>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub description: String,
    pub generator: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub device_uuid: [u8; 6],
    /// Identifies arrival-clock origin/unit and sensor-clock relationship, including unknown relationships.
    pub clock_basis: String,
    pub settings: GeneralSettings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Screen {
    pub id: u8,
    pub calibration: ScreenCalibration<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trace {
    pub schema_version: u32,
    pub name: String,
    pub provenance: Provenance,
    pub scene: Option<SceneDescription>,
    pub sources: Vec<Source>,
    pub screens: Vec<Screen>,
    pub samples: Vec<Sample>,
    pub thresholds: Thresholds,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    /// Arrival-clock microseconds since trace origin. Equal times preserve array order.
    pub elapsed_us: u64,
    pub source_id: String,
    pub input: Input,
    /// Independent truth in client coordinates; never inferred from tracker state.
    pub truth: Option<Truth>,
    /// Expected valid raycast, not whether marker pixels exist. None means unspecified.
    pub expectation: Option<bool>,
    pub score: bool,
    /// Metadata only: classical replay consumes raw reports, not these images.
    pub image_artifacts: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Imu {
        timestamp: u32,
        accel: [f32; 3],
        gyro: [f32; 3],
    },
    Poc {
        points: [[u16; 2]; 16],
    },
    Combined {
        nf_points: [[u16; 2]; 16],
        wf_points: [[u16; 2]; 16],
    },
    Reset,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    pub min_availability: f64,
    pub min_truth_matches: usize,
    pub max_aim_rmse: Option<f64>,
    pub max_position_rmse_m: Option<f64>,
    pub max_jump: Option<f64>,
    pub max_reacquisition_us: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub schema_version: u32,
    pub trace_name: String,
    /// SHA256 of the trace's canonical CBOR encoding ([`to_cbor`]), including
    /// settings, truth and limits; equals the SHA256 of a `.cbor` trace file
    /// written by this crate.
    pub trace_sha256: String,
    pub backend: String,
    /// Exactly one entry per trace sample, including reset and missing outputs.
    pub outputs: Vec<Output>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub sample_index: usize,
    pub tracking: Option<Tracked>,
    /// Preserves non-finite output failures without attempting invalid JSON floats.
    pub non_finite: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tracked {
    pub timestamp: u32,
    pub aimpoint: [f64; 2],
    pub position_m: [f64; 3],
    /// Row-major rotation in the public client coordinate convention.
    pub rotation: [[f64; 3]; 3],
    pub distance_m: f64,
    pub screen_id: u32,
}

impl Tracked {
    fn finite(&self) -> bool {
        self.aimpoint
            .iter()
            .chain(&self.position_m)
            .chain(self.rotation.iter().flatten())
            .chain(std::iter::once(&self.distance_m))
            .all(|v| v.is_finite())
    }

    fn from_event(event: TrackingEvent) -> Self {
        Self {
            timestamp: event.timestamp,
            aimpoint: [f64::from(event.aimpoint.x), f64::from(event.aimpoint.y)],
            position_m: event.pose.translation.map(f64::from).into(),
            rotation: std::array::from_fn(|r| {
                std::array::from_fn(|c| f64::from(event.pose.rotation[(r, c)]))
            }),
            distance_m: f64::from(event.distance),
            screen_id: event.screen_id,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccuracyStatus {
    Checked,
    NotAccuracyChecked,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metrics {
    pub sample_count: usize,
    pub scored_samples: usize,
    pub available_samples: usize,
    pub availability: f64,
    pub truth_samples: usize,
    pub truth_matches: usize,
    pub position_matches: usize,
    pub aim_rmse: Option<f64>,
    pub position_rmse_m: Option<f64>,
    pub max_jump: Option<f64>,
    pub non_finite_outputs: usize,
    pub expectation_mismatches: usize,
    pub screen_mismatches: usize,
    /// Arrival-clock time from start of an expected-valid segment to its first valid output.
    pub max_reacquisition_us: Option<u64>,
    pub unreacquired_segments: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub schema_version: u32,
    pub trace_name: String,
    pub backend: String,
    pub provenance: Provenance,
    pub passed: bool,
    pub accuracy_status: AccuracyStatus,
    pub metrics: Metrics,
    pub violations: Vec<String>,
    pub capture: Capture,
}

fn require(condition: bool, message: impl Into<String>) -> Result<(), Error> {
    if condition {
        Ok(())
    } else {
        Err(Error::Invalid(message.into()))
    }
}

fn text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 4096
}

fn contains_null(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::Array(values) => values.iter().any(contains_null),
        serde_json::Value::Object(values) => values.values().any(contains_null),
        _ => false,
    }
}

/// Also reject unknown nested fields in reused configuration types which predate strict traces.
fn known_fields(input: &serde_json::Value, canonical: &serde_json::Value) -> Result<(), Error> {
    match (input, canonical) {
        (serde_json::Value::Object(input), serde_json::Value::Object(canonical)) => {
            for (key, value) in input {
                let expected = canonical
                    .get(key)
                    .ok_or_else(|| Error::Invalid(format!("unknown field {key}")))?;
                known_fields(value, expected)?;
            }
        }
        (serde_json::Value::Array(input), serde_json::Value::Array(canonical)) => {
            for (value, expected) in input.iter().zip(canonical) {
                known_fields(value, expected)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn read_bounded(reader: impl Read) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    reader.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
    require(
        bytes.len() as u64 <= MAX_INPUT_BYTES,
        "input exceeds 64 MiB",
    )?;
    Ok(bytes)
}

/// Bounded JSON reader rejecting unknown and duplicate fields, including reused settings.
pub fn read_json<T: serde::de::DeserializeOwned + Serialize>(
    reader: impl Read,
) -> Result<T, Error> {
    let bytes = read_bounded(reader)?;
    let value: T = serde_json::from_slice(&bytes)?;
    known_fields(
        &serde_json::from_slice(&bytes)?,
        &serde_json::to_value(&value)?,
    )?;
    Ok(value)
}

/// Bounded CBOR reader rejecting unknown and duplicate fields and trailing data.
pub fn read_cbor<T: serde::de::DeserializeOwned + Serialize>(
    reader: impl Read,
) -> Result<T, Error> {
    let bytes = read_bounded(reader)?;
    let mut decoder = minicbor_serde::Deserializer::new(&bytes);
    let value = T::deserialize(&mut decoder)?;
    require(
        decoder.decoder().position() == bytes.len(),
        "trailing CBOR data",
    )?;
    // Reused configuration types can have different JSON and CBOR representations.
    known_fields(
        &minicbor_serde::from_slice(&bytes)?,
        &minicbor_serde::from_slice(&to_cbor(&value)?)?,
    )?;
    Ok(value)
}

/// Canonical CBOR: string-keyed maps in field order, floats at their declared width.
pub fn to_cbor(value: &impl Serialize) -> Result<Vec<u8>, Error> {
    Ok(minicbor_serde::to_vec(value)?)
}

impl Trace {
    pub fn validate(&self) -> Result<(), Error> {
        require(
            self.schema_version == SCHEMA_VERSION,
            "unsupported trace schema_version",
        )?;
        require(
            text(&self.name) && text(&self.provenance.description),
            "name and provenance description required",
        )?;
        require(
            self.provenance.generator.as_deref().is_none_or(text),
            "invalid provenance generator",
        )?;
        require(
            !self.sources.is_empty() && self.sources.len() <= MAX_SOURCES,
            "need 1..=32 sources",
        )?;
        require(
            !self.samples.is_empty() && self.samples.len() <= MAX_SAMPLES,
            "need 1..=100000 samples",
        )?;
        require(
            !self.screens.is_empty() && self.screens.len() <= 6,
            "need 1..=6 screen calibrations",
        )?;
        let mut ids = HashSet::new();
        let mut settings_by_device = HashMap::new();
        for source in &self.sources {
            require(
                text(&source.id) && text(&source.clock_basis),
                "source id and clock_basis required",
            )?;
            require(ids.insert(source.id.as_str()), "duplicate source id")?;
            let settings = serde_json::to_value(&source.settings)?;
            require(!contains_null(&settings), "non-finite settings")?;
            if let Some(previous) = settings_by_device.get(&source.device_uuid) {
                require(
                    previous == &settings,
                    "sources for same device UUID have conflicting settings",
                )?;
            } else {
                settings_by_device.insert(source.device_uuid, settings);
            }
            for camera in [
                &source.settings.camera_model_nf,
                &source.settings.camera_model_wf,
            ] {
                require(
                    camera.fx() > 0.0 && camera.fy() > 0.0,
                    "camera focal lengths must be positive",
                )?;
                require(
                    camera.k.determinant().abs() > f32::EPSILON
                        && camera.rect.determinant().abs() > f32::EPSILON,
                    "singular camera calibration",
                )?;
            }
            require(
                (source.settings.stereo_iso.rotation.norm_squared() - 1.0).abs() < 1e-3,
                "invalid stereo rotation",
            )?;
        }
        let mut screens = HashSet::new();
        for screen in &self.screens {
            require(
                screen.id <= odyssey_common::MAX_SCREEN_ID && screens.insert(u32::from(screen.id)),
                "duplicate or invalid screen id",
            )?;
            screen
                .calibration
                .validate()
                .map_err(|e| Error::Invalid(e.to_string()))?;
            let p = &screen.calibration.object_points;
            require(
                p.iter()
                    .enumerate()
                    .all(|(i, a)| p[..i].iter().all(|b| (a - b).norm_squared() > 1e-12)),
                "duplicate screen markers",
            )?;
            require(
                p.iter().all(|p| p.z.abs() <= 1e-4),
                "screen markers must lie on XY plane",
            )?;
            require(
                (p[1] - p[0]).cross(&(p[2] - p[0])).norm_squared() > 1e-12,
                "first three screen markers must not be collinear",
            )?;
        }
        let limits = &self.thresholds;
        require(
            limits.min_availability.is_finite() && (0.0..=1.0).contains(&limits.min_availability),
            "min_availability must be finite in [0,1]",
        )?;
        for limit in [
            limits.max_aim_rmse,
            limits.max_position_rmse_m,
            limits.max_jump,
        ]
        .into_iter()
        .flatten()
        {
            require(
                limit.is_finite() && limit >= 0.0,
                "limits must be finite and nonnegative",
            )?;
        }
        require(
            (limits.max_aim_rmse.is_none() && limits.max_position_rmse_m.is_none())
                || limits.min_truth_matches > 0,
            "accuracy limits require min_truth_matches > 0",
        )?;
        let mut previous = 0;
        let mut scored = 0;
        for (index, sample) in self.samples.iter().enumerate() {
            require(
                sample.elapsed_us >= previous,
                format!("sample {index}: arrival time decreased"),
            )?;
            previous = sample.elapsed_us;
            require(
                ids.contains(sample.source_id.as_str()),
                format!("sample {index}: unknown source"),
            )?;
            require(
                sample.image_artifacts.len() <= 16
                    && sample.image_artifacts.iter().all(|p| text(p)),
                "invalid image artifact references",
            )?;
            if let Input::Imu { accel, gyro, .. } = &sample.input {
                require(
                    accel.iter().chain(gyro).all(|v| v.is_finite()),
                    format!("sample {index}: non-finite IMU"),
                )?;
            }
            if matches!(sample.input, Input::Reset) {
                require(
                    sample.truth.is_none() && sample.expectation.is_none(),
                    "reset cannot carry truth or validity expectation",
                )?;
            } else if sample.score {
                scored += 1;
            }
            if let Some(truth) = &sample.truth {
                require(
                    truth
                        .aimpoint
                        .iter()
                        .chain(truth.position_m.iter().flatten())
                        .all(|v| v.is_finite()),
                    "non-finite truth",
                )?;
                require(
                    screens.contains(&truth.screen_id),
                    "truth references unknown screen",
                )?;
                require(
                    sample.expectation != Some(false),
                    "truth conflicts with expected-invalid sample",
                )?;
            }
        }
        require(scored > 0, "trace has no scored sensor samples")?;
        require(
            limits.min_availability > 0.0
                || limits.min_truth_matches > 0
                || limits.max_aim_rmse.is_some()
                || limits.max_position_rmse_m.is_some()
                || limits.max_jump.is_some()
                || limits.max_reacquisition_us.is_some()
                || self
                    .samples
                    .iter()
                    .any(|s| s.score && s.expectation.is_some()),
            "trace has no active limits or validity expectations",
        )?;
        if let Some(scene) = &self.scene {
            scene
                .validate()
                .map_err(|error| Error::Invalid(error.to_string()))?;
            require(
                scene
                    .cameras
                    .iter()
                    .all(|c| ids.contains(c.source_id.as_str())),
                "scene camera references unknown source",
            )?;
            require(
                scene.screens.len() == self.screens.len()
                    && scene.screens.iter().all(|scene_screen| {
                        self.screens.iter().any(|screen| {
                            scene_screen.id == screen.id
                                && scene_screen.calibration.rotation == screen.calibration.rotation
                                && scene_screen.calibration.homography
                                    == screen.calibration.homography
                                && scene_screen.calibration.object_points
                                    == screen.calibration.object_points
                        })
                    }),
                "scene and tracker screen calibrations must match exactly",
            )?;
        }
        Ok(())
    }

    pub fn sha256(&self) -> Result<String, Error> {
        Ok(hex::encode(Sha256::digest(to_cbor(self)?)))
    }
}

/// Replay exact array order through production DeviceTracker, with no sleeps or wall-clock dependencies.
pub async fn replay(trace: &Trace) -> Result<Report, Error> {
    trace.validate()?;
    let screens: ScreenCalibrationList = trace
        .screens
        .iter()
        .map(|s| (s.id, s.calibration.clone()))
        .collect();
    let (message_channel, _messages) = mpsc::channel(1);
    let (event_sender, mut events) = broadcast::channel(8);
    let context = DeviceTaskContext {
        message_channel,
        screen_calibrations: Arc::new(ArcSwap::from_pointee(screens)),
        device_offsets: Arc::new(Mutex::new(HashMap::new())),
        device_shot_delays: Arc::new(RwLock::new(HashMap::new())),
        shot_delay_watch: Arc::new(RwLock::new(HashMap::new())),
        event_sender,
        direct_usb_handler_uuids: Arc::new(Mutex::new(HashSet::new())),
        bond_names: Arc::new(Mutex::new(HashMap::new())),
    };
    let mut trackers = HashMap::new();
    let sources: HashMap<_, _> = trace.sources.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut outputs = Vec::with_capacity(trace.samples.len());
    for (sample_index, sample) in trace.samples.iter().enumerate() {
        let source = sources[sample.source_id.as_str()];
        let tracker = trackers
            .entry(source.device_uuid)
            .or_insert_with(|| DeviceTracker::new(Some(&source.settings)));
        let device = Device {
            uuid: source.device_uuid,
            transport: Transport::Usb,
            capabilities: DeviceCapabilities::new(DeviceCapabilities::EVENTS),
            firmware_version: [0; 3],
            events_transport: EventsTransport::Wired,
            events_connected: true,
            product_id: 0,
            name: Device::name_bytes(&source.id),
            netcore_release: 0,
        };
        let event = match &sample.input {
            Input::Imu {
                timestamp,
                accel,
                gyro,
            } => Some(DeviceStreamEvent::Accel(AccelReport {
                timestamp: *timestamp,
                accel: Vector3::from(*accel),
                gyro: Vector3::from(*gyro),
            })),
            Input::Poc { points } => Some(DeviceStreamEvent::PocMarkers(PocMarkersReport {
                points: points.map(Point2::from),
            })),
            Input::Combined {
                nf_points,
                wf_points,
            } => Some(DeviceStreamEvent::CombinedMarkers(CombinedMarkersReport {
                nf_points: nf_points.map(Point2::from),
                wf_points: wf_points.map(Point2::from),
            })),
            Input::Reset => {
                *tracker = DeviceTracker::new(Some(&source.settings));
                None
            }
        };
        if let Some(event) = event {
            tracker.handle_event(&device, event, &context).await;
        }
        let mut output = Output {
            sample_index,
            tracking: None,
            non_finite: false,
        };
        loop {
            match events.try_recv() {
                Ok(Event::DeviceEvent(DeviceEvent(_, DeviceEventKind::TrackingEvent(event)))) => {
                    require(
                        output.tracking.is_none() && !output.non_finite,
                        "multiple tracking outputs for one sample",
                    )?;
                    let tracking = Tracked::from_event(event);
                    if tracking.finite() {
                        output.tracking = Some(tracking);
                    } else {
                        output.non_finite = true;
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(error) => {
                    return Err(Error::Invalid(format!("lost production events: {error}")));
                }
            }
        }
        outputs.push(output);
    }
    evaluate(
        trace,
        &Capture {
            schema_version: SCHEMA_VERSION,
            trace_name: trace.name.clone(),
            trace_sha256: trace.sha256()?,
            backend: "production_device_tracker".into(),
            outputs,
        },
    )
}

/// Score real captures or replay outputs against the same independently supplied truth and limits.
pub fn evaluate(trace: &Trace, capture: &Capture) -> Result<Report, Error> {
    trace.validate()?;
    require(
        capture.schema_version == SCHEMA_VERSION,
        "unsupported capture schema_version",
    )?;
    require(
        capture.trace_name == trace.name && capture.trace_sha256 == trace.sha256()?,
        "capture/trace identity mismatch",
    )?;
    require(text(&capture.backend), "capture backend required")?;
    require(
        capture.outputs.len() == trace.samples.len(),
        "capture must contain one output per sample",
    )?;
    let mut m = Metrics {
        sample_count: trace.samples.len(),
        scored_samples: 0,
        available_samples: 0,
        availability: 0.0,
        truth_samples: 0,
        truth_matches: 0,
        position_matches: 0,
        aim_rmse: None,
        position_rmse_m: None,
        max_jump: None,
        non_finite_outputs: 0,
        expectation_mismatches: 0,
        screen_mismatches: 0,
        max_reacquisition_us: None,
        unreacquired_segments: 0,
    };
    let mut aim_squared = 0.0;
    let mut position_squared = 0.0;
    let mut availability_samples = 0;
    let mut previous: HashMap<[u8; 6], &Tracked> = HashMap::new();
    let mut expected = HashMap::new();
    let mut pending = HashMap::new();
    let devices: HashMap<_, _> = trace
        .sources
        .iter()
        .map(|s| (s.id.as_str(), s.device_uuid))
        .collect();
    let screens: HashSet<_> = trace.screens.iter().map(|s| u32::from(s.id)).collect();
    for (index, (sample, output)) in trace.samples.iter().zip(&capture.outputs).enumerate() {
        let device_uuid = devices[sample.source_id.as_str()];
        require(
            output.sample_index == index,
            "capture outputs must be ordered exact sample indices",
        )?;
        require(
            !(output.non_finite && output.tracking.is_some()),
            "non-finite flag conflicts with finite tracking output",
        )?;
        if let Some(tracking) = &output.tracking {
            require(
                tracking.finite(),
                "capture contains non-finite tracking values",
            )?;
            require(
                screens.contains(&tracking.screen_id),
                "capture output references unknown screen",
            )?;
            require(tracking.distance_m >= 0.0, "negative tracking distance")?;
            let rotation = nalgebra::Matrix3::from_fn(|r, c| tracking.rotation[r][c]);
            require(
                (rotation.determinant() - 1.0).abs() <= 1e-3
                    && (rotation.transpose() * rotation - nalgebra::Matrix3::identity()).norm()
                        <= 1e-3,
                "capture rotation must be orthonormal with determinant +1",
            )?;
        }
        m.non_finite_outputs += usize::from(output.non_finite);
        if matches!(sample.input, Input::Reset) {
            require(
                output.tracking.is_none() && !output.non_finite,
                "reset produced tracking output",
            )?;
            previous.remove(&device_uuid);
            expected.remove(&device_uuid);
            m.unreacquired_segments += usize::from(pending.remove(&device_uuid).is_some());
            continue;
        }
        if !sample.score {
            continue;
        }
        m.scored_samples += 1;
        let valid = output.tracking.as_ref().filter(|t| t.distance_m > 0.0);
        if sample.expectation != Some(false) {
            availability_samples += 1;
            m.available_samples += usize::from(valid.is_some());
        }
        if let Some(wanted) = sample.expectation {
            m.expectation_mismatches += usize::from(wanted != valid.is_some());
            let was_expected = expected.insert(device_uuid, wanted).unwrap_or(false);
            if wanted && !was_expected {
                pending.insert(device_uuid, sample.elapsed_us);
            }
            if !wanted {
                m.unreacquired_segments += usize::from(pending.remove(&device_uuid).is_some());
            }
        }
        if let Some(tracking) = valid {
            if let Some(start) = pending.remove(&device_uuid) {
                let delay = sample.elapsed_us - start;
                m.max_reacquisition_us = Some(m.max_reacquisition_us.unwrap_or(0).max(delay));
            }
            if let Some(last) = previous.insert(device_uuid, tracking)
                && last.screen_id == tracking.screen_id
            {
                let jump = distance_squared(&last.aimpoint, &tracking.aimpoint).sqrt();
                m.max_jump = Some(m.max_jump.unwrap_or(0.0).max(jump));
            }
        } else {
            previous.remove(&device_uuid);
        }
        if let Some(truth) = &sample.truth {
            m.truth_samples += 1;
            if let Some(tracking) = valid {
                if tracking.screen_id == truth.screen_id {
                    m.truth_matches += 1;
                    aim_squared += distance_squared(&tracking.aimpoint, &truth.aimpoint);
                    if let Some(position) = truth.position_m {
                        m.position_matches += 1;
                        position_squared += distance_squared(&tracking.position_m, &position);
                    }
                } else {
                    m.screen_mismatches += 1;
                }
            }
        }
    }
    m.unreacquired_segments += pending.len();
    if availability_samples > 0 {
        m.availability = m.available_samples as f64 / f64::from(availability_samples);
    }
    if m.truth_matches > 0 {
        m.aim_rmse = Some((aim_squared / m.truth_matches as f64).sqrt());
    }
    if m.position_matches > 0 {
        m.position_rmse_m = Some((position_squared / m.position_matches as f64).sqrt());
    }
    let t = &trace.thresholds;
    let mut violations = Vec::new();
    if m.non_finite_outputs > 0 {
        violations.push(format!("{} non-finite outputs", m.non_finite_outputs));
    }
    if m.expectation_mismatches > 0 {
        violations.push(format!(
            "{} validity expectation mismatches",
            m.expectation_mismatches
        ));
    }
    if m.screen_mismatches > 0 {
        violations.push(format!("{} truth screen mismatches", m.screen_mismatches));
    }
    if m.availability < t.min_availability {
        violations.push(format!(
            "availability {} < {}",
            m.availability, t.min_availability
        ));
    }
    if m.truth_matches < t.min_truth_matches {
        violations.push(format!(
            "truth_matches {} < {}",
            m.truth_matches, t.min_truth_matches
        ));
    }
    for (name, measured, limit) in [
        ("aim_rmse", m.aim_rmse, t.max_aim_rmse),
        ("position_rmse_m", m.position_rmse_m, t.max_position_rmse_m),
        ("max_jump", m.max_jump, t.max_jump),
    ] {
        if let Some(limit) = limit {
            match measured {
                Some(value) if value.is_finite() && value <= limit => {}
                _ => violations.push(format!(
                    "{name} {measured:?} exceeds limit {limit} or lacks observations"
                )),
            }
        }
    }
    if let Some(limit) = t.max_reacquisition_us
        && (m.unreacquired_segments > 0 || m.max_reacquisition_us.is_none_or(|time| time > limit))
    {
        violations.push(format!(
            "reacquisition {:?} us, {} unreacquired segments; limit {limit}",
            m.max_reacquisition_us, m.unreacquired_segments
        ));
    }
    require(
        aim_squared.is_finite()
            && position_squared.is_finite()
            && m.max_jump.is_none_or(f64::is_finite),
        "error metric overflow",
    )?;
    Ok(Report {
        schema_version: SCHEMA_VERSION,
        trace_name: trace.name.clone(),
        backend: capture.backend.clone(),
        provenance: trace.provenance.clone(),
        passed: violations.is_empty(),
        accuracy_status: if m.truth_matches > 0 {
            AccuracyStatus::Checked
        } else {
            AccuracyStatus::NotAccuracyChecked
        },
        metrics: m,
        violations,
        capture: capture.clone(),
    })
}

fn distance_squared<const N: usize>(a: &[f64; N], b: &[f64; N]) -> f64 {
    a.iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}

#[cfg(test)]
mod tests;
