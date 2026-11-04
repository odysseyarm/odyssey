//! Accelerometer bias/scale and gyroscope bias calibration.

use argmin::core::observers::ObserverMode;
use argmin::core::{CostFunction, Error, Executor, Gradient};
use argmin_observer_slog::SlogLogger;
use futures::StreamExt as _;
use nalgebra::Vector3;
use odyssey_usb::device::VmDevice;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, BufReader};

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

/// Blocks until the user presses Enter.
async fn wait_for_enter() -> Result<(), String> {
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut input = String::new();
    reader
        .read_line(&mut input)
        .await
        .map_err(|e| format!("Failed to read input: {e}"))?;
    Ok(())
}

fn save_calibration(path: &str, calibration: &impl Serialize) -> Result<(), String> {
    let data = serde_json::to_value(calibration)
        .map_err(|e| format!("Unable to serialize calibration: {e}"))?;
    let text = serde_json::to_string_pretty(&data)
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
    for orientation in &ACCEL_ORIENTATIONS {
        println!("{orientation}");
        println!("Press Enter when ready to start collecting samples for this orientation.");
        wait_for_enter().await?;

        let mut samples = device
            .stream_accel()
            .await
            .map_err(|e| format!("Failed to start accel stream: {e}"))?;
        for _ in 0..num_samples {
            if let Some(sample) = samples.next().await {
                measurements.push(sample.accel.cast::<f64>());
            }
        }

        println!("Collected samples for this orientation.");
    }

    // Validate measurements
    measurements.retain(|m| !m.x.is_nan() && !m.y.is_nan() && !m.z.is_nan());

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
        .best_param
        .ok_or_else(|| "Optimization returned no solution".to_string())?;
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
    wait_for_enter().await?;

    let mut samples = device
        .stream_accel()
        .await
        .map_err(|e| format!("Failed to start accel stream: {e}"))?;

    let mut gyro_sum = Vector3::<f64>::zeros();
    for _ in 0..num_samples {
        if let Some(sample) = samples.next().await {
            gyro_sum += sample.gyro.cast::<f64>();
        }
    }
    let gyro_bias = gyro_sum / (num_samples as f64);

    let calibration = GyroCalibration {
        b_x: gyro_bias.x,
        b_y: gyro_bias.y,
        b_z: gyro_bias.z,
    };
    save_calibration(gyro_path, &calibration)?;
    println!("Gyroscope bias saved to {gyro_path}");

    Ok(())
}
