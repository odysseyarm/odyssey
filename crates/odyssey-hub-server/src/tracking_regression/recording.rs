use std::path::Path;

use odyssey_usb::packets::vm::PacketData;
use serde::{Deserialize, Serialize};

use super::{
    Error, Input, MAX_INPUT_BYTES, Provenance, SCHEMA_VERSION, Sample, Screen, Source, Thresholds,
    Trace,
};

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingTemplate {
    /// Since: Odyssey Desktop 0.7.0.
    pub name: String,
    /// Since: Odyssey Desktop 0.7.0.
    pub source: RecordingSource,
    /// Since: Odyssey Desktop 0.7.0.
    pub screens: Vec<Screen>,
    /// Since: Odyssey Desktop 0.7.0.
    pub thresholds: Thresholds,
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingSource {
    /// Since: Odyssey Desktop 0.7.0.
    pub id: String,
    /// Since: Odyssey Desktop 0.7.0.
    pub device_uuid: [u8; 6],
}

/// Imports one device's legacy recording using template identity, screens and limits.
///
/// Arrival stamps are Unix milliseconds. Sensor timestamps stay unchanged.
///
/// # Errors
/// Returns an error for I/O or playback decoding failures, oversized or empty
/// recordings, backward or overflowing arrival timestamps, or an invalid trace.
///
/// Since: Odyssey Desktop 0.7.0.
pub fn import_recording(path: &Path, template: RecordingTemplate) -> Result<Trace, Error> {
    if std::fs::metadata(path)?.len() > MAX_INPUT_BYTES {
        return Err(Error::Invalid("recording exceeds 64 MiB".into()));
    }
    let (settings, packets) = odyssey_playback::read_file(path)?;
    let origin = packets
        .first()
        .ok_or_else(|| Error::Invalid("empty recording".into()))?
        .0;
    let mut previous = origin;
    let source_id = template.source.id;
    let mut samples = Vec::new();
    for (arrival_ms, packet) in packets {
        if arrival_ms < previous {
            return Err(Error::Invalid("recording arrival clock decreased".into()));
        }
        previous = arrival_ms;
        let elapsed_us = arrival_ms
            .checked_sub(origin)
            .and_then(|value| value.checked_mul(1000))
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| Error::Invalid("recording timestamp overflow".into()))?;
        let input = match packet.data {
            PacketData::AccelReport(report) => Input::Imu {
                timestamp: report.timestamp,
                accel: report.accel.into(),
                gyro: report.gyro.into(),
            },
            PacketData::MarkersReport(report) => Input::Markers {
                points: report.points.map(|point| [point.x, point.y]),
            },
            PacketData::LegacyCombinedMarkersReport(report) => Input::Combined {
                nf_points: report.nf_points.map(|point| [point.x, point.y]),
                wf_points: report.wf_points.map(|point| [point.x, point.y]),
            },
            _ => continue,
        };
        samples.push(Sample {
            elapsed_us,
            source_id: source_id.clone(),
            input,
            truth: None,
            expectation: None,
            score: true,
            image_artifacts: Vec::new(),
        });
    }
    let trace = Trace {
        schema_version: SCHEMA_VERSION,
        name: template.name,
        provenance: Provenance {
            description: format!(
                "physical recording {}; template supplies identity, screens and limits, not ground truth",
                path.display()
            ),
            generator: None,
        },
        scene: None,
        sources: vec![Source {
            id: source_id,
            device_uuid: template.source.device_uuid,
            settings,
            clock_basis: "legacy recorder Unix milliseconds normalized to arrival elapsed_us; IMU timestamp is device microseconds; clocks unsynchronized".into(),
        }],
        screens: template.screens,
        samples,
        thresholds: template.thresholds,
    };
    trace.validate()?;
    Ok(trace)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use odyssey_usb::packets::vm::Packet;

    use super::*;

    fn recording(stamps: &[u128]) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "tracking-recording-{}-{}.bin",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let settings = odyssey_usb::device::GeneralSettings::default();
        let mut bytes = postcard::to_stdvec(&settings).expect("settings encode");
        for &stamp in stamps {
            bytes.extend_from_slice(&stamp.to_le_bytes());
            bytes.extend(
                postcard::to_stdvec(&Packet {
                    id: 0,
                    data: PacketData::MarkersReport(odyssey_usb::packets::vm::MarkersReport {
                        points: [nalgebra::Point2::origin(); 16],
                    }),
                })
                .expect("packet encodes"),
            );
        }
        std::fs::write(&path, bytes).expect("writes fixture");
        path
    }

    fn template() -> RecordingTemplate {
        let trace = super::super::scenarios::builtin_scenarios().remove(0);
        RecordingTemplate {
            name: trace.name,
            source: RecordingSource {
                id: trace.sources[0].id.clone(),
                device_uuid: trace.sources[0].device_uuid,
            },
            screens: trace.screens,
            thresholds: trace.thresholds,
        }
    }

    #[test]
    fn import_preserves_arrival_spacing_without_fabricating_truth() {
        let template =
            super::super::read_json(serde_json::to_vec(&template()).unwrap().as_slice()).unwrap();
        let path = recording(&[1_000, 1_000, 1_007]);
        let imported = import_recording(&path, template).expect("imports recording");
        std::fs::remove_file(path).expect("removes fixture");
        assert_eq!(
            imported
                .samples
                .iter()
                .map(|s| s.elapsed_us)
                .collect::<Vec<_>>(),
            [0, 0, 7_000]
        );
        assert!(imported.scene.is_none());
        assert!(
            imported
                .samples
                .iter()
                .all(|s| s.truth.is_none() && s.expectation.is_none())
        );
    }

    #[test]
    fn template_rejects_settings_and_trace_fields() {
        let mut json = serde_json::to_value(template()).unwrap();
        json["source"]["settings"] = serde_json::json!({});
        assert!(matches!(
            super::super::read_json::<RecordingTemplate>(
                serde_json::to_vec(&json).unwrap().as_slice(),
            ),
            Err(Error::Json(_))
        ));
        let trace = super::super::scenarios::builtin_scenarios().remove(0);
        assert!(matches!(
            super::super::read_json::<RecordingTemplate>(
                serde_json::to_vec(&trace).unwrap().as_slice(),
            ),
            Err(Error::Json(_))
        ));
    }

    #[test]
    fn import_rejects_backward_wall_clock() {
        let template = template();
        let path = recording(&[1_000, 999]);
        let imported = import_recording(&path, template);
        std::fs::remove_file(path).expect("removes fixture");
        assert!(matches!(imported, Err(Error::Invalid(_))));
    }
}
