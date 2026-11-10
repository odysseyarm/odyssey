//! Replays a staged run through the production tracker.

use odyssey_hub_server::tracking_regression::staged::trace_from_scene;
use odyssey_hub_server::tracking_regression::{Input, Metrics, replay, to_cbor};
use odyssey_stage::Compiled;
use serde::Serialize;

use crate::blender::export_dir;
use crate::error::ApiError;

/// Files of the replay in the formats the Rerun exporter reads.
#[derive(Serialize)]
pub(crate) struct RerunFiles {
    trace_path: String,
    capture_path: String,
    command: String,
}
#[derive(Serialize)]
pub(crate) struct Point {
    t_s: f64,
    truth_uv: [f64; 2],
    tracked_uv: Option<[f64; 2]>,
    error: Option<f64>,
}

#[derive(Serialize)]
pub(crate) struct Evaluation {
    passed: bool,
    violations: Vec<String>,
    metrics: Metrics,
    tracker_knows_surface: bool,
    rerun: RerunFiles,
    series: Vec<Point>,
}

pub(crate) async fn evaluate(
    compiled: &Compiled,
    device: &str,
    fps: f64,
    tracker_knows_surface: bool,
) -> Result<Evaluation, ApiError> {
    let scene = compiled.scene(device, fps)?;
    let trace = trace_from_scene(&format!("staged-{device}"), scene, tracker_knows_surface)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let report = replay(&trace)
        .await
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let mut series = Vec::new();
    for (sample, output) in trace.samples.iter().zip(&report.capture.outputs) {
        let (Input::Poc { .. }, Some(truth)) = (&sample.input, &sample.truth) else {
            continue;
        };
        let tracked_uv = output.tracking.as_ref().map(|tracked| tracked.aimpoint);
        series.push(Point {
            t_s: sample.elapsed_us as f64 / 1_000_000.0,
            truth_uv: truth.aimpoint,
            error: tracked_uv.map(|tracked| {
                (tracked[0] - truth.aimpoint[0]).hypot(tracked[1] - truth.aimpoint[1])
            }),
            tracked_uv,
        });
    }
    let dir = export_dir()?;
    let name = format!("staged-{device}");
    let write = |suffix: &str, bytes: Vec<u8>| -> Result<String, ApiError> {
        let path = dir.join(format!("{name}-{suffix}.cbor"));
        std::fs::write(&path, bytes).map_err(|error| {
            ApiError::Internal(format!("cannot write {}: {error}", path.display()))
        })?;
        Ok(path.display().to_string())
    };
    let cbor = |error: odyssey_hub_server::tracking_regression::Error| {
        ApiError::Internal(error.to_string())
    };
    let trace_path = write("trace", to_cbor(&trace).map_err(cbor)?)?;
    let capture_path = write("capture", to_cbor(&report.capture).map_err(cbor)?)?;
    let rrd = dir.join(format!("{name}-replay.rrd")).display().to_string();
    Ok(Evaluation {
        passed: report.passed,
        violations: report.violations,
        metrics: report.metrics,
        tracker_knows_surface,
        series,
        rerun: RerunFiles {
            command: format!(
                "python3 tools/tracking_scene/rerun_export.py {trace_path} {rrd} --capture {capture_path} --validator \"$VALIDATOR\" --self-check && rerun {rrd}"
            ),
            trace_path,
            capture_path,
        },
    })
}

#[cfg(test)]
mod tests {
    use odyssey_stage::pattern::Pattern;
    use odyssey_stage::{DeviceKind, Stage, display_preset};

    use super::*;

    #[tokio::test]
    async fn a_tracker_that_ignores_the_wavy_surface_aims_worse_than_one_that_knows_it() {
        let mut stage = Stage {
            display: display_preset("tarp-on-a-wall").unwrap(),
            duration_s: 2.0,
            ..Stage::default()
        };
        stage.devices[0].position_m = [1.2, 0.7, -2.5];
        if let DeviceKind::Tracker(tracker) = &mut stage.devices[0].kind {
            tracker.pattern = Pattern::Hold { at: [0.3, 0.4] };
        }
        let compiled = stage.compile().unwrap();
        let rmse = |evaluation: Evaluation| evaluation.metrics.aim_rmse.unwrap();
        let aware = rmse(evaluate(&compiled, "tracker-1", 30.0, true).await.unwrap());
        let unaware = rmse(evaluate(&compiled, "tracker-1", 30.0, false).await.unwrap());
        assert!(unaware > 2.0 * aware, "aware {aware}, unaware {unaware}");
    }
}
