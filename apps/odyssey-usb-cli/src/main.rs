#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "command-line tool: stdout/stderr is its interface"
)]

use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use futures::StreamExt as _;
use nalgebra::Vector3;
use nusb::MaybeFuture as _;
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::AccelReport;

mod calibration;
mod serial_ports;
mod usb_ids;

/// Weight of the running average when folding in a new sample interval.
const INTERVAL_KEEP_WEIGHT: f64 = 0.7 / 0.8;
/// Weight of the newest sample interval in the running average.
const INTERVAL_NEW_WEIGHT: f64 = 0.1 / 0.8;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Serial port path
    #[arg(short, long)]
    path: Option<String>,

    /// Output file for accelerometer bias and scale calculation
    #[arg(long)]
    accel: Option<String>,

    /// Output file for gyroscope bias calculation
    #[arg(long)]
    gyro: Option<String>,

    /// Number of samples to use for bias and scale calculation
    #[arg(short = 'n', value_name = "N", default_value_t = 400)]
    num_samples: usize,

    /// Gravity value to use for calculations
    #[arg(short = 'g', value_name = "G", default_value_t = 9.80665)]
    gravity: f64,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The serial port the user asked for, or the only one that is connected.
/// Explains and returns `None` when it cannot be determined.
fn resolve_serial_path(requested: Option<String>) -> Option<String> {
    if requested.is_some() {
        return requested;
    }

    let mut ports = serial_ports::list_device_serial_ports();
    if ports.is_empty() {
        eprintln!("No devices connected");
        return None;
    }
    if ports.len() > 1 {
        eprintln!("Multiple devices detected, please specify one: {ports:?}");
        return None;
    }
    Some(ports.remove(0))
}

async fn run(args: Args) -> Result<(), String> {
    let Some(path) = resolve_serial_path(args.path) else {
        return Ok(());
    };
    println!("Connecting to device at {path}");

    let device_info = nusb::list_devices()
        .wait()
        .map_err(|e| format!("Failed to list USB devices: {e}"))?
        .find(|info| usb_ids::is_vm_device(info.vendor_id(), info.product_id()));
    let Some(device_info) = device_info else {
        eprintln!("No USB device found");
        return Ok(());
    };

    let mut device = VmDevice::connect_usb(device_info)
        .await
        .map_err(|e| format!("Failed to connect to device: {e}"))?;
    println!("Connected to device");

    if let Some(gyro_path) = args.gyro {
        calibration::calculate_and_save_gyro_bias(&mut device, &gyro_path, args.num_samples).await
    } else if let Some(accel_path) = args.accel {
        calibration::calculate_and_save_bias_scale(
            &mut device,
            &accel_path,
            args.num_samples,
            args.gravity,
        )
        .await
    } else {
        let mut samples = device
            .stream_accel()
            .await
            .map_err(|e| format!("Failed to start accel stream: {e}"))?;
        normal_streaming(&mut samples).await
    }
}

/// Running estimate of the sample rate (ODR), from wall-clock arrival times.
#[derive(Default)]
struct SampleRateEstimator {
    last_arrival: Option<Instant>,
    average_interval: Option<f64>,
}

impl SampleRateEstimator {
    /// Records one sample arrival and returns the estimated rate in Hz, once
    /// there are at least two samples.
    fn record_sample(&mut self) -> Option<f64> {
        let now = Instant::now();
        if let Some(last_arrival) = self.last_arrival.replace(now) {
            let interval = now.duration_since(last_arrival).as_secs_f64();
            self.average_interval = Some(match self.average_interval {
                None => interval,
                Some(average) => INTERVAL_KEEP_WEIGHT * average + INTERVAL_NEW_WEIGHT * interval,
            });
        }
        self.average_interval.map(|interval| 1.0 / interval)
    }
}

async fn normal_streaming(
    samples: &mut (impl futures::Stream<Item = AccelReport> + Unpin),
) -> Result<(), String> {
    let mut accel_sum = Vector3::<f64>::zeros();
    let mut gyro_sum = Vector3::<f64>::zeros();
    let mut count = 0.0;

    let stream_ended = tokio::select! {
        _ = tokio::signal::ctrl_c() => false,
        () = async {
            let mut rate = SampleRateEstimator::default();
            let mut prev_timestamp = None;
            while let Some(sample) = samples.next().await {
                let output_data_rate = rate.record_sample();
                let elapsed = prev_timestamp.map_or(0, |prev| sample.timestamp - prev);
                prev_timestamp = Some(sample.timestamp);

                println!(
                    "accel = {:8.4?}, ||accel|| = {:7.4}, gyro = {:8.4?}, ts = {:9}, elapsed = {:7}, ODR = {:7.3?}",
                    sample.accel,
                    sample.accel.magnitude(),
                    sample.gyro,
                    sample.timestamp,
                    elapsed,
                    output_data_rate
                );
                count += 1.0;
                accel_sum += sample.accel.cast::<f64>();
                gyro_sum += sample.gyro.cast::<f64>();
            }
        } => true,
    };

    println!();
    println!("Mean accel: {:.8?}", accel_sum / count);
    println!(
        "Mean accel magnitude: {:.8?}",
        (accel_sum / count).magnitude()
    );
    println!("Mean gyro: {:.8?}", gyro_sum / count);
    println!("Samples: {count}");

    if stream_ended {
        return Err("Accel stream ended unexpectedly".to_string());
    }
    Ok(())
}
