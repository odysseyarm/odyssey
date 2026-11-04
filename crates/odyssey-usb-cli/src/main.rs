#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "command-line tool: stdout/stderr is its interface"
)]

use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use eros::Context as _;
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
    /// Select a device by its CDC ACM serial port path (otherwise require exactly one USB device)
    #[arg(short, long)]
    path: Option<String>,

    /// Output file for accelerometer bias and scale calculation
    #[arg(long, conflicts_with = "gyro")]
    accel: Option<String>,

    /// Output file for gyroscope bias calculation
    #[arg(long, conflicts_with = "accel")]
    gyro: Option<String>,

    /// Positive number of samples per calibration orientation
    #[arg(short = 'n', value_name = "N", default_value_t = 400)]
    num_samples: usize,

    /// Finite, positive gravity value to use for accelerometer calibration
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

/// Match the requested serial port to the same physical USB device, or require
/// exactly one direct USB device when no path was given.
fn resolve_usb_device(requested: Option<&str>) -> eros::Result<nusb::DeviceInfo> {
    let identity = requested.map(serial_ports::usb_identity).transpose()?;
    let mut devices = nusb::list_devices()
        .wait()
        .context("Failed to list USB devices")?
        .filter(|info| usb_ids::is_vm_device(info.vendor_id(), info.product_id()))
        .filter(|info| {
            identity.as_ref().is_none_or(|identity| {
                info.vendor_id() == identity.vid
                    && info.product_id() == identity.pid
                    && info.serial_number() == identity.serial_number.as_deref()
            })
        });
    let device = devices
        .next()
        .ok_or_else(|| eros::error!("No matching Odyssey USB device found"))?;
    if devices.next().is_some() {
        eros::bail!("Multiple matching USB devices found; select a unique device with --path");
    }
    Ok(device)
}

async fn run(args: Args) -> eros::Result<()> {
    if args.accel.is_some() || args.gyro.is_some() {
        calibration::validate_sample_count(args.num_samples)?;
    }
    if args.accel.is_some() {
        calibration::validate_gravity(args.gravity)?;
    }
    let device_info = resolve_usb_device(args.path.as_deref())?;
    println!(
        "Connecting to USB device {}",
        device_info.serial_number().unwrap_or("(no serial number)")
    );

    let mut device = VmDevice::connect_usb(device_info)
        .await
        .context("Failed to connect to device")?;
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
            .context("Failed to start accel stream")?;
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
) -> eros::Result<()> {
    let mut accel_sum = Vector3::<f64>::zeros();
    let mut gyro_sum = Vector3::<f64>::zeros();
    let mut count = 0.0;

    let stream_ended = tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            signal.context("Failed to listen for Ctrl+C")?;
            false
        },
        () = async {
            let mut rate = SampleRateEstimator::default();
            let mut prev_timestamp = None;
            while let Some(sample) = samples.next().await {
                let output_data_rate = rate.record_sample();
                let elapsed = prev_timestamp.map_or(0, |prev| sample.timestamp.wrapping_sub(prev));
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
        eros::bail!("Accel stream ended unexpectedly");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_output_modes_are_mutually_exclusive() {
        assert!(
            Args::try_parse_from([
                "odyssey-usb-cli",
                "--accel",
                "accel.json",
                "--gyro",
                "gyro.json"
            ])
            .is_err()
        );
    }

    #[tokio::test]
    async fn timestamp_rollover_reaches_the_stream_end_error() {
        let mut samples = futures::stream::iter([
            AccelReport {
                timestamp: u32::MAX,
                ..AccelReport::default()
            },
            AccelReport {
                timestamp: 0,
                ..AccelReport::default()
            },
        ]);
        assert!(normal_streaming(&mut samples).await.is_err());
    }
}
