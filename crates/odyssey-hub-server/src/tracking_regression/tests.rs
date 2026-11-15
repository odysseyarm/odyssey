use super::*;

fn trace() -> Trace {
    Trace {
        schema_version: SCHEMA_VERSION,
        name: "oracle-contract".into(),
        provenance: Provenance {
            description: "Independent oracle unit fixture".into(),
            generator: None,
        },
        scene: None,
        sources: vec![Source {
            id: "unit".into(),
            device_uuid: [1; 6],
            clock_basis: "host arrival microseconds; unrelated sensor counter".into(),
            settings: GeneralSettings::default(),
        }],
        screens: vec![Screen {
            id: 0,
            calibration: odyssey_sim::synthetic_screen_calibration(),
        }],
        samples: (0..2)
            .map(|index| Sample {
                elapsed_us: index * 10_000,
                source_id: "unit".into(),
                input: Input::Markers {
                    points: [[0; 2]; 16],
                },
                truth: Some(Truth {
                    aimpoint: [0.0; 2],
                    position_m: Some([0.0; 3]),
                    screen_id: 0,
                }),
                expectation: None,
                score: true,
                image_artifacts: vec![],
            })
            .collect(),
        thresholds: Thresholds {
            min_availability: 1.0,
            min_truth_matches: 2,
            max_aim_rmse: Some(0.4),
            max_position_rmse_m: Some(1.0),
            max_jump: Some(0.6),
            max_reacquisition_us: None,
        },
    }
}

fn tracked(aimpoint: [f64; 2], position_m: [f64; 3]) -> Tracked {
    Tracked {
        timestamp: 0,
        aimpoint,
        position_m,
        rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        distance_m: 2.0,
        screen_id: 0,
    }
}

fn capture(trace: &Trace, values: Vec<Option<Tracked>>) -> Capture {
    Capture {
        schema_version: SCHEMA_VERSION,
        trace_name: trace.name.clone(),
        trace_sha256: trace.sha256().unwrap(),
        backend: "independent_bench_capture".into(),
        outputs: values
            .into_iter()
            .enumerate()
            .map(|(sample_index, tracking)| Output {
                sample_index,
                tracking,
                non_finite: false,
            })
            .collect(),
    }
}

#[test]
fn independent_errors_and_jumps_use_exact_sample_matches() {
    let trace = trace();
    let capture = capture(
        &trace,
        vec![
            Some(tracked([0.3, 0.4], [0.0, 0.0, 1.0])),
            Some(tracked([0.0; 2], [0.0; 3])),
        ],
    );
    let report = evaluate(&trace, &capture).unwrap();
    assert!(report.passed);
    assert_eq!(report.metrics.truth_matches, 2);
    assert_eq!(report.metrics.availability, 1.0);
    assert!((report.metrics.aim_rmse.unwrap() - (0.125_f64).sqrt()).abs() < 1e-12);
    assert!((report.metrics.position_rmse_m.unwrap() - (0.5_f64).sqrt()).abs() < 1e-12);
    assert_eq!(report.metrics.max_jump, Some(0.5));

    let mut corrupted = capture;
    corrupted.outputs[1].tracking.as_mut().unwrap().aimpoint = [9.0, 9.0];
    let failed = evaluate(&trace, &corrupted).unwrap();
    assert!(!failed.passed);
    assert!(failed.violations.iter().any(|v| v.starts_with("aim_rmse")));
    assert!(failed.violations.iter().any(|v| v.starts_with("max_jump")));
}

#[test]
fn unavailable_truth_is_not_an_accuracy_pass() {
    let mut trace = trace();
    for sample in &mut trace.samples {
        sample.truth = None;
    }
    trace.thresholds.min_truth_matches = 0;
    trace.thresholds.max_aim_rmse = None;
    trace.thresholds.max_position_rmse_m = None;
    let available = capture(
        &trace,
        vec![
            Some(tracked([0.0; 2], [0.0; 3])),
            Some(tracked([0.0; 2], [0.0; 3])),
        ],
    );
    let structural = evaluate(&trace, &available).unwrap();
    assert!(structural.passed);
    assert_eq!(
        structural.accuracy_status,
        AccuracyStatus::NotAccuracyChecked
    );
    let capture = capture(&trace, vec![None, None]);
    let report = evaluate(&trace, &capture).unwrap();
    assert_eq!(report.accuracy_status, AccuracyStatus::NotAccuracyChecked);
    assert_eq!(report.metrics.aim_rmse, None);
    assert_eq!(report.metrics.availability, 0.0);
    assert!(!report.passed);
}

#[test]
fn trace_identity_order_and_reset_outputs_are_enforced() {
    let mut trace = trace();
    let mut capture = capture(&trace, vec![None, None]);
    capture.outputs[0].sample_index = 1;
    assert!(
        evaluate(&trace, &capture)
            .unwrap_err()
            .to_string()
            .contains("sample indices")
    );
    capture.outputs[0].sample_index = 0;
    trace.samples[1].elapsed_us += 1;
    assert!(
        evaluate(&trace, &capture)
            .unwrap_err()
            .to_string()
            .contains("identity mismatch")
    );
    trace.samples[1].input = Input::Reset;
    trace.samples[1].truth = None;
    capture.trace_sha256 = trace.sha256().unwrap();
    capture.outputs[1].tracking = Some(tracked([0.0; 2], [0.0; 3]));
    assert!(
        evaluate(&trace, &capture)
            .unwrap_err()
            .to_string()
            .contains("reset produced")
    );
}

#[test]
fn non_finite_output_is_recorded_as_failure_not_missing_accuracy() {
    let trace = trace();
    let mut capture = capture(&trace, vec![None, None]);
    capture.outputs[0].non_finite = true;
    let report = evaluate(&trace, &capture).unwrap();
    assert_eq!(report.metrics.non_finite_outputs, 1);
    assert!(!report.passed);
    capture.outputs[0].non_finite = false;
    capture.outputs[0].tracking = Some(tracked([f64::NAN, 0.0], [0.0; 3]));
    assert!(
        evaluate(&trace, &capture)
            .unwrap_err()
            .to_string()
            .contains("non-finite")
    );
}

#[test]
fn capture_rotations_reject_scaling_and_reflection() {
    let trace = trace();
    let mut capture = capture(&trace, vec![Some(tracked([0.0; 2], [0.0; 3])), None]);
    capture.outputs[0].tracking.as_mut().unwrap().rotation[0][0] = 2.0;
    assert!(
        evaluate(&trace, &capture)
            .unwrap_err()
            .to_string()
            .contains("orthonormal")
    );
    capture.outputs[0].tracking.as_mut().unwrap().rotation[0][0] = -1.0;
    assert!(
        evaluate(&trace, &capture)
            .unwrap_err()
            .to_string()
            .contains("determinant +1")
    );
}

#[test]
fn recovery_uses_arrival_clock_and_reset_breaks_jump_history() {
    let mut trace = trace();
    trace.samples[0].expectation = Some(true);
    trace.samples[1].expectation = Some(true);
    trace.samples[1].elapsed_us = 123_456;
    trace.thresholds.max_reacquisition_us = Some(123_455);
    let captured = capture(&trace, vec![None, Some(tracked([0.0; 2], [0.0; 3]))]);
    let report = evaluate(&trace, &captured).unwrap();
    assert_eq!(report.metrics.max_reacquisition_us, Some(123_456));
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.starts_with("reacquisition"))
    );

    let mut reset = trace.samples[0].clone();
    reset.elapsed_us = 1;
    reset.input = Input::Reset;
    reset.truth = None;
    reset.expectation = None;
    trace.samples.insert(1, reset);
    let capture = capture(
        &trace,
        vec![
            Some(tracked([0.0; 2], [0.0; 3])),
            None,
            Some(tracked([8.0; 2], [0.0; 3])),
        ],
    );
    let report = evaluate(&trace, &capture).unwrap();
    assert_eq!(report.metrics.max_jump, None);
}

#[test]
fn malformed_trace_and_nested_unknown_settings_are_rejected() {
    let mut trace = trace();
    trace.samples[0].elapsed_us = 10_001;
    assert!(
        trace
            .validate()
            .unwrap_err()
            .to_string()
            .contains("arrival time decreased")
    );
    trace.samples[0].elapsed_us = trace.samples[1].elapsed_us;
    trace.validate().unwrap();
    trace.thresholds.max_aim_rmse = Some(f64::NAN);
    assert!(trace.validate().unwrap_err().to_string().contains("limits"));
    trace.thresholds.max_aim_rmse = Some(0.4);
    trace.samples[0].input = Input::Imu {
        timestamp: 4,
        accel: [0.0, f32::INFINITY, 0.0],
        gyro: [0.0; 3],
    };
    assert!(
        trace
            .validate()
            .unwrap_err()
            .to_string()
            .contains("non-finite IMU")
    );
    trace.samples[0].input = Input::Markers {
        points: [[0; 2]; 16],
    };
    let mut json = serde_json::to_value(&trace).unwrap();
    json["sources"][0]["settings"]["misspelled_focal_length"] = serde_json::json!(50.0);
    assert!(
        read_json::<Trace>(serde_json::to_vec(&json).unwrap().as_slice())
            .unwrap_err()
            .to_string()
            .contains("unknown field")
    );
    let duplicate = serde_json::to_string(&trace).unwrap().replacen(
        "\"schema_version\":1",
        "\"schema_version\":1,\"schema_version\":1",
        1,
    );
    assert!(
        read_json::<Trace>(duplicate.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("duplicate field")
    );
    trace.samples.clear();
    assert!(
        trace
            .validate()
            .unwrap_err()
            .to_string()
            .contains("samples")
    );
}

#[tokio::test]
async fn mixed_sources_fuse_by_device_and_distinct_devices_stay_isolated() {
    let mut baseline = scenarios::builtin_scenarios().remove(0);
    let elapsed_us = baseline.samples.last().unwrap().elapsed_us + 1;
    let mut reset = baseline.samples[0].clone();
    reset.elapsed_us = elapsed_us;
    reset.input = Input::Reset;
    reset.truth = None;
    reset.expectation = None;
    reset.score = false;
    let mut after_reset = reset.clone();
    after_reset.elapsed_us += 1;
    after_reset.input = baseline.samples[0].input.clone();
    baseline.samples.extend([reset, after_reset]);
    let single = replay(&baseline).await.unwrap();
    assert!(single.passed, "{:?}", single.violations);
    assert!(
        single
            .capture
            .outputs
            .iter()
            .zip(&baseline.samples)
            .any(|(o, s)| matches!(s.input, Input::Imu { .. }) && o.tracking.is_some())
    );

    let mut mixed = baseline.clone();
    let mut imu_source = mixed.sources[0].clone();
    imu_source.id = "independent-imu-clock".into();
    imu_source.clock_basis =
        "physical IMU arrival clock aligned to trace origin; sensor counter independent".into();
    for sample in &mut mixed.samples {
        if matches!(sample.input, Input::Imu { .. }) {
            sample.source_id = imu_source.id.clone();
        }
    }
    mixed.sources.push(imu_source);
    let fused = replay(&mixed).await.unwrap();
    assert_eq!(
        serde_json::to_value(&single.capture.outputs).unwrap(),
        serde_json::to_value(&fused.capture.outputs).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&single.metrics).unwrap(),
        serde_json::to_value(&fused.metrics).unwrap()
    );
    assert!(fused.capture.outputs.last().unwrap().tracking.is_none());

    let mut conflicting = mixed.clone();
    conflicting.sources[1]
        .settings
        .stereo_iso
        .translation
        .vector
        .x += 0.01;
    assert!(
        conflicting
            .validate()
            .unwrap_err()
            .to_string()
            .contains("conflicting settings")
    );

    mixed.sources[1].device_uuid = [0x7f; 6];
    let separate = replay(&mixed).await.unwrap();
    for (sample, output) in mixed.samples.iter().zip(&separate.capture.outputs) {
        if matches!(sample.input, Input::Imu { .. }) {
            assert!(
                output.tracking.is_none(),
                "other unit's optics leaked into IMU-only unit"
            );
        }
    }
    assert!(
        separate
            .capture
            .outputs
            .iter()
            .zip(&mixed.samples)
            .any(|(o, s)| matches!(s.input, Input::Markers { .. }) && o.tracking.is_some())
    );
}

#[test]
fn cbor_traces_round_trip_canonically_and_stay_strict() {
    let trace = trace();
    let bytes = to_cbor(&trace).unwrap();
    let decoded: Trace = read_cbor(bytes.as_slice()).unwrap();
    assert_eq!(to_cbor(&decoded).unwrap(), bytes);
    assert_eq!(hex::encode(Sha256::digest(&bytes)), trace.sha256().unwrap());
    assert!(matches!(
        read_cbor::<Trace>(serde_json::to_vec(&trace).unwrap().as_slice()),
        Err(Error::CborDecode(_))
    ));
    assert!(matches!(
        read_json::<Trace>(bytes.as_slice()),
        Err(Error::Json(_))
    ));
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(
        read_cbor::<Trace>(trailing.as_slice())
            .unwrap_err()
            .to_string()
            .contains("trailing")
    );

    // Insert an extra entry at the start of the nested `settings` map, whose
    // reused type does not deny unknown fields on its own.
    let key = b"\x68settings";
    let at = bytes
        .windows(key.len())
        .position(|window| window == key)
        .unwrap()
        + key.len();
    let mut unknown = bytes.clone();
    assert!((0xa0..0xb7).contains(&unknown[at]), "{:#x}", unknown[at]);
    unknown[at] += 1;
    let mut entry = vec![0x77];
    entry.extend_from_slice(b"misspelled_focal_length");
    entry.push(0x01);
    unknown.splice((at + 1)..=at, entry);
    let error = read_cbor::<Trace>(unknown.as_slice()).unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");

    // A short map header (0xa0 + n) has room for one more entry: repeat a key.
    let mut duplicate = bytes;
    assert!((0xa0..0xb7).contains(&duplicate[0]), "{:#x}", duplicate[0]);
    duplicate[0] += 1;
    duplicate.extend_from_slice(&[0x6e]);
    duplicate.extend_from_slice(b"schema_version");
    duplicate.push(0x01);
    let error = read_cbor::<Trace>(duplicate.as_slice()).unwrap_err();
    assert!(error.to_string().contains("duplicate field"), "{error}");
}

#[test]
fn document_readers_enforce_size_limit() {
    let oversized = || std::io::repeat(b' ').take(MAX_INPUT_BYTES + 1);
    assert!(
        read_json::<Trace>(oversized())
            .unwrap_err()
            .to_string()
            .contains("64 MiB")
    );
    assert!(
        read_cbor::<Trace>(oversized())
            .unwrap_err()
            .to_string()
            .contains("64 MiB")
    );
}
