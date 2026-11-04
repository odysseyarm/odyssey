//! Accelerometer and gyroscope calibration.

use argmin::core::{CostFunction, Error, Executor, State as _};
use futures::StreamExt as _;
use nalgebra::Vector3;
use odyssey_usb::device::VmDevice;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, BufReader};

/// Nelder-Mead gives up after this many iterations.
const MAX_OPTIMIZER_ITERATIONS: u64 = 1000;
/// Offset applied to each parameter in turn to build the initial simplex.
const SIMPLEX_STEP: f64 = 0.05;

/// One prompt per device orientation; the six faces give the optimizer enough
/// distinct gravity directions to separate bias from scale.
const ACCEL_ORIENTATIONS: [&str; 6] = [
    "Place the device with the top side facing up.",
    "Place the device with the bottom side facing up.",
    "Place the device with the front side facing up.",
    "Place the device with the back side facing up.",
    "Place the device with the left side facing up.",
    "Place the device with the right side facing up.",
];

#[derive(Serialize, Deserialize)]
struct AccelCalibration {
    b_x: f64,
    b_y: f64,
    b_z: f64,
    s_x: f64,
    s_y: f64,
    s_z: f64,
}

#[derive(Serialize, Deserialize)]
struct GyroCalibration {
    b_x: f64,
    b_y: f64,
    b_z: f64,
}

/// Sum of squared deviations of the scaled and biased accelerometer magnitude
/// from gravity. Parameters are `[b_x, b_y, b_z, s_x, s_y, s_z]`.
struct CostFn {
    measurements: Vec<Vector3<f64>>,
    gravity: f64,
}

impl CostFunction for CostFn {
    type Param = Vec<f64>;

    type Output = f64;

    fn cost(&self, p: &Self::Param) -> Result<Self::Output, Error> {
        let [b_x, b_y, b_z, s_x, s_y, s_z] = [p[0], p[1], p[2], p[3], p[4], p[5]];

        let mut cost = 0.0;
        for m in &self.measurements {
            let ax = s_x * m.x + b_x;
            let ay = s_y * m.y + b_y;
            let az = s_z * m.z + b_z;
            let diff = ax * ax + ay * ay + az * az - self.gravity * self.gravity;
            cost += diff * diff;
        }

        Ok(cost)
    }
}

/// Keep one reader across prompts so buffered input is not lost. EOF is not
/// confirmation: unattended commands must not emit an unobserved calibration.
async fn wait_for_enter(reader: &mut (impl AsyncBufRead + Unpin)) -> Result<(), String> {
    let mut input = String::new();
    let bytes = reader
        .read_line(&mut input)
        .await
        .map_err(|e| format!("Failed to read input: {e}"))?;
    if bytes == 0 {
        return Err("Input closed before calibration was confirmed".to_string());
    }
    Ok(())
}

pub(crate) fn validate_sample_count(num_samples: usize) -> Result<(), String> {
    if num_samples == 0 {
        return Err("Calibration requires at least one sample".to_string());
    }
    Ok(())
}

pub(crate) fn validate_gravity(gravity: f64) -> Result<(), String> {
    if !gravity.is_finite() || gravity <= 0.0 {
        return Err("Gravity must be finite and positive".to_string());
    }
    Ok(())
}

fn save_calibration(output_path: &str, calibration: &impl Serialize) -> Result<(), String> {
    let text = serde_json::to_string_pretty(calibration)
        .map_err(|e| format!("Unable to serialize calibration: {e}"))?;
    std::fs::write(output_path, text).map_err(|e| format!("Unable to write file: {e}"))
}

pub(crate) async fn cmd_accel_calib(
    device: &mut VmDevice,
    num_samples: usize,
    gravity: f64,
    output_path: &str,
) -> Result<(), String> {
    let mut measurements = Vec::new();
    let mut input = BufReader::new(tokio::io::stdin());
    for orientation in &ACCEL_ORIENTATIONS {
        println!("{orientation}");
        println!("Press Enter when ready to start collecting samples for this orientation.");
        wait_for_enter(&mut input).await?;

        let mut samples = device
            .stream_accel()
            .await
            .map_err(|e| format!("Failed to start accel stream: {e}"))?;
        for _ in 0..num_samples {
            let sample = samples.next().await.ok_or_else(|| {
                "Accel stream ended before all samples were collected".to_string()
            })?;
            if !sample.accel.iter().all(|value| value.is_finite()) {
                return Err("Accelerometer sample contains a non-finite value".to_string());
            }
            measurements.push(sample.accel.cast::<f64>());
        }

        println!("Collected samples for this orientation.");
    }

    println!(
        "Running optimization with {} measurements...",
        measurements.len()
    );

    let cost_function = CostFn {
        measurements,
        gravity,
    };

    // Zero bias, unit scale.
    let init_param = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

    // Nelder-Mead starts from n + 1 vertices around the initial guess.
    let mut simplex = Vec::with_capacity(init_param.len() + 1);
    simplex.push(init_param.clone());
    for i in 0..init_param.len() {
        let mut vertex = init_param.clone();
        vertex[i] += SIMPLEX_STEP;
        simplex.push(vertex);
    }

    let solver = argmin::solver::neldermead::NelderMead::new(simplex);
    let executor = Executor::new(cost_function, solver).configure(|state| {
        state
            .param(init_param.clone())
            .max_iters(MAX_OPTIMIZER_ITERATIONS)
    });
    let result = executor
        .run()
        .map_err(|e| format!("Optimization failed: {e}"))?;

    let state = result.state;
    let solution = state
        .get_best_param()
        .or_else(|| state.get_param())
        .ok_or_else(|| "Optimization returned no solution".to_string())?;
    if !state.get_best_cost().is_finite()
        || !solution.iter().all(|value| value.is_finite())
        || solution[3..].iter().any(|scale| *scale <= 0.0)
    {
        return Err(
            "Optimization did not produce a finite, positive-scale calibration".to_string(),
        );
    }

    println!("Optimization complete!");
    println!("  Final cost: {}", state.get_cost());
    println!("  Iterations: {}", state.get_iter());

    let calibration = AccelCalibration {
        b_x: solution[0],
        b_y: solution[1],
        b_z: solution[2],
        s_x: solution[3],
        s_y: solution[4],
        s_z: solution[5],
    };
    save_calibration(output_path, &calibration)?;
    println!("Bias and scale saved to {output_path}");

    Ok(())
}

pub(crate) async fn cmd_gyro_calib(
    device: &mut VmDevice,
    num_samples: usize,
    output_path: &str,
) -> Result<(), String> {
    println!("Please make sure the device is held firmly in place.");
    println!("Press Enter when ready to start collecting samples for gyroscope calibration.");
    wait_for_enter(&mut BufReader::new(tokio::io::stdin())).await?;

    let mut samples = device
        .stream_accel()
        .await
        .map_err(|e| format!("Failed to start accel stream: {e}"))?;

    let gyro_bias = collect_gyro_bias(&mut samples, num_samples).await?;

    let calibration = GyroCalibration {
        b_x: gyro_bias.x,
        b_y: gyro_bias.y,
        b_z: gyro_bias.z,
    };
    save_calibration(output_path, &calibration)?;
    println!("Gyroscope bias saved to {output_path}");

    Ok(())
}

/// Require every requested sample: dividing a short stream by the requested
/// count would underestimate the bias.
async fn collect_gyro_bias(
    samples: &mut (impl futures::Stream<Item = odyssey_usb::packets::vm::AccelReport> + Unpin),
    num_samples: usize,
) -> Result<Vector3<f64>, String> {
    validate_sample_count(num_samples)?;
    let mut sum = Vector3::<f64>::zeros();
    for _ in 0..num_samples {
        let sample = samples
            .next()
            .await
            .ok_or_else(|| "Gyro stream ended before all samples were collected".to_string())?;
        if !sample.gyro.iter().all(|value| value.is_finite()) {
            return Err("Gyroscope sample contains a non-finite value".to_string());
        }
        sum += sample.gyro.cast::<f64>();
    }
    Ok(sum / num_samples as f64)
}

pub(crate) async fn cmd_stream(device: &mut VmDevice) -> Result<(), String> {
    let mut samples = device
        .stream_accel()
        .await
        .map_err(|e| format!("Failed to start accel stream: {e}"))?;

    let mut accel_sum = Vector3::<f64>::zeros();
    let mut gyro_sum = Vector3::<f64>::zeros();
    let mut count = 0.0;

    println!("Streaming data... Press Ctrl+C to stop.");

    let stream_ended = tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|e| format!("Failed to listen for Ctrl+C: {e}"))?;
            false
        },
        () = async {
            let mut prev_timestamp = None;
            while let Some(sample) = samples.next().await {
                let elapsed = prev_timestamp.map_or(0, |prev| sample.timestamp.wrapping_sub(prev));
                prev_timestamp = Some(sample.timestamp);

                println!(
                    "accel = {:8.4?}, ||accel|| = {:7.4}, gyro = {:8.4?}, ts = {:9}, elapsed = {:7}",
                    sample.accel,
                    sample.accel.magnitude(),
                    sample.gyro,
                    sample.timestamp,
                    elapsed
                );

                count += 1.0;
                accel_sum += sample.accel.cast::<f64>();
                gyro_sum += sample.gyro.cast::<f64>();
            }
        } => true,
    };

    println!();
    if count > 0.0 {
        println!("Mean accel: {:.8?}", accel_sum / count);
        println!(
            "Mean accel magnitude: {:.8?}",
            (accel_sum / count).magnitude()
        );
        println!("Mean gyro: {:.8?}", gyro_sum / count);
    }
    println!("Samples: {count}");

    if stream_ended {
        return Err("Accel stream ended unexpectedly".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use odyssey_usb::packets::vm::AccelReport;

    use super::*;

    #[tokio::test]
    async fn prompts_preserve_buffered_confirmations_and_reject_eof() {
        let mut reader = BufReader::new(&b"\n\n"[..]);
        assert!(wait_for_enter(&mut reader).await.is_ok());
        assert!(wait_for_enter(&mut reader).await.is_ok());
        assert!(wait_for_enter(&mut reader).await.is_err());
    }

    #[tokio::test]
    async fn gyro_bias_requires_complete_finite_samples() {
        let first = AccelReport {
            gyro: Vector3::new(1.0, -2.0, 3.0),
            ..AccelReport::default()
        };
        let second = AccelReport {
            gyro: Vector3::new(3.0, -4.0, 5.0),
            ..AccelReport::default()
        };
        let mut complete = futures::stream::iter([first, second]);
        assert_eq!(
            collect_gyro_bias(&mut complete, 2).await.ok(),
            Some(Vector3::new(2.0, -3.0, 4.0))
        );
        let mut short = futures::stream::iter([first]);
        assert!(collect_gyro_bias(&mut short, 2).await.is_err());
        let mut empty = futures::stream::empty();
        assert!(collect_gyro_bias(&mut empty, 0).await.is_err());
        let invalid = AccelReport {
            gyro: Vector3::new(f32::INFINITY, 0.0, 0.0),
            ..AccelReport::default()
        };
        let mut non_finite = futures::stream::iter([invalid]);
        assert!(collect_gyro_bias(&mut non_finite, 1).await.is_err());
    }

    #[test]
    fn gravity_must_be_finite_and_positive() {
        for gravity in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(validate_gravity(gravity).is_err());
        }
        assert!(validate_gravity(9.80665).is_ok());
    }
}
