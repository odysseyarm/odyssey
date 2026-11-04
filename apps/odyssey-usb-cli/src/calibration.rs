//! Accelerometer bias/scale and gyroscope bias calibration.

use argmin::core::observers::ObserverMode;
use argmin::core::{CostFunction, Error, Executor, Gradient, State as _};
use argmin_observer_slog::SlogLogger;
use futures::StreamExt as _;
use nalgebra::Vector3;
use odyssey_usb::device::VmDevice;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, BufReader};

/// The optimizer gives up after this many iterations.
const MAX_OPTIMIZER_ITERATIONS: u64 = 1000;

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
            cost += (ax * ax + ay * ay + az * az - self.gravity * self.gravity).powi(2);
        }

        Ok(cost)
    }
}

impl Gradient for CostFn {
    type Param = Vec<f64>;
    type Gradient = Vec<f64>;

    fn gradient(&self, p: &Self::Param) -> Result<Self::Gradient, Error> {
        let [b_x, b_y, b_z, s_x, s_y, s_z] = [p[0], p[1], p[2], p[3], p[4], p[5]];

        let mut grad = vec![0.0; 6];
        for m in &self.measurements {
            let ax = s_x * m.x + b_x;
            let ay = s_y * m.y + b_y;
            let az = s_z * m.z + b_z;
            let common_term = 4.0 * (ax * ax + ay * ay + az * az - self.gravity * self.gravity);
            grad[0] += common_term * ax;
            grad[1] += common_term * ay;
            grad[2] += common_term * az;
            grad[3] += common_term * ax * m.x;
            grad[4] += common_term * ay * m.y;
            grad[5] += common_term * az * m.z;
        }

        Ok(grad)
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

fn save_calibration(path: &str, calibration: &impl Serialize) -> Result<(), String> {
    let text = serde_json::to_string_pretty(calibration)
        .map_err(|e| format!("Unable to serialize calibration: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("Unable to write file: {e}"))
}

pub(crate) async fn calculate_and_save_bias_scale(
    device: &mut VmDevice,
    accel_path: &str,
    num_samples: usize,
    gravity: f64,
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

    let cost_function = CostFn {
        measurements,
        gravity,
    };

    // Initial parameter guess: [b_x, b_y, b_z, s_x, s_y, s_z]
    let init_param = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

    // Perform optimization with detailed logging
    let solver = argmin::solver::gradientdescent::SteepestDescent::new(
        argmin::solver::linesearch::MoreThuenteLineSearch::new(),
    );
    let result = Executor::new(cost_function, solver)
        .configure(|state| state.param(init_param).max_iters(MAX_OPTIMIZER_ITERATIONS))
        .add_observer(SlogLogger::term(), ObserverMode::Always)
        .run()
        .map_err(|e| format!("Optimization failed: {e}"))?;

    let solution = result
        .state
        .get_best_param()
        .ok_or_else(|| "Optimization returned no solution".to_string())?;
    if !result.state.get_best_cost().is_finite()
        || !solution.iter().all(|value| value.is_finite())
        || solution[3..].iter().any(|scale| *scale <= 0.0)
    {
        return Err(
            "Optimization did not produce a finite, positive-scale calibration".to_string(),
        );
    }
    let calibration = AccelCalibration {
        b_x: solution[0],
        b_y: solution[1],
        b_z: solution[2],
        s_x: solution[3],
        s_y: solution[4],
        s_z: solution[5],
    };
    save_calibration(accel_path, &calibration)?;
    println!("Bias and scale saved to {accel_path}");

    Ok(())
}

pub(crate) async fn calculate_and_save_gyro_bias(
    device: &mut VmDevice,
    gyro_path: &str,
    num_samples: usize,
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
    save_calibration(gyro_path, &calibration)?;
    println!("Gyroscope bias saved to {gyro_path}");

    Ok(())
}

/// Require every requested sample; dividing a short stream by the requested
/// count would silently underestimate the bias.
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
