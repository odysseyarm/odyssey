//! Records the device's raw packet streams to a file that `odyssey-playback`
//! can read (see `odyssey_vision_tool::recording` for the layout).

#![expect(
    clippy::print_stderr,
    reason = "command-line tool: stderr is its interface"
)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::UNIX_EPOCH;

use anyhow::{Context as _, Result};
use clap::Parser;
use nusb::DeviceInfo;
use odyssey_protocol::control::device::TransportMode;
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::{PacketData, PacketType};
use odyssey_vision_tool::recording::encode_recording;
use tokio::sync::mpsc::{self, Sender};
use tokio_stream::StreamExt as _;

/// Nordic Semiconductor's USB vendor id.
const NORDIC_VENDOR_ID: u16 = 0x1915;
/// Product ids of the devices that can be recorded.
const RECORDABLE_PRODUCT_IDS: [u16; 3] = [0x520F, 0x5210, 0x5211];

/// Capacity of the channel carrying "Enter was pressed" events.
const STDIN_EVENT_CAPACITY: usize = 50;

#[derive(Parser)]
#[command(about = "Record a USB device's raw packets for odyssey-playback")]
struct Args {
    /// Output recording file.
    output: PathBuf,
}

async fn get_devices() -> Result<Vec<DeviceInfo>> {
    let devices = nusb::list_devices()
        .await
        .context("failed to list USB devices")?;

    let mut filtered = Vec::new();
    for info in devices.into_iter().filter(|info| {
        info.vendor_id() == NORDIC_VENDOR_ID && RECORDABLE_PRODUCT_IDS.contains(&info.product_id())
    }) {
        match VmDevice::probe_transport_mode(&info).await {
            Ok(TransportMode::Usb) => filtered.push(info),
            Ok(mode) => eprintln!(
                "Skipping {:04x}:{:04x} - transport mode {:?}",
                info.vendor_id(),
                info.product_id(),
                mode
            ),
            Err(e) => eprintln!(
                "Skipping {:04x}:{:04x} - failed to probe mode: {e}",
                info.vendor_id(),
                info.product_id()
            ),
        }
    }

    Ok(filtered)
}

/// Sends one event per line read from stdin. Runs on its own thread because
/// stdin reads block.
fn stdin_thread(sender: Sender<()>) {
    loop {
        match std::io::stdin().read_line(&mut String::new()) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) => {
                eprintln!("Failed to read stdin: {e}");
                return;
            }
        }
        if sender.blocking_send(()).is_err() {
            return;
        }
    }
}

/// Records packets, each stamped with the wall-clock time it arrived, until
/// Enter is pressed again.
async fn record_packets(
    device: &VmDevice,
    stdin_line: &mut mpsc::Receiver<()>,
) -> Result<Vec<(u128, PacketData)>> {
    let accel_stream = device
        .stream(PacketType::AccelReport())
        .await
        .context("failed to start accel stream")?;
    let mot_stream = device
        .stream(PacketType::ObjectReport())
        .await
        .context("failed to start object report stream")?;
    let comb_stream = device
        .stream(PacketType::CombinedMarkersReport())
        .await
        .context("failed to start combined markers stream")?;
    let poc_stream = device
        .stream(PacketType::PocMarkersReport())
        .await
        .context("failed to start POC markers stream")?;
    let impact_stream = device
        .stream(PacketType::ImpactReport())
        .await
        .context("failed to start impact stream")?;
    let mut merged_stream = accel_stream
        .merge(mot_stream)
        .merge(comb_stream)
        .merge(poc_stream)
        .merge(impact_stream);

    let mut packets = vec![];
    loop {
        tokio::select! {
            _ = stdin_line.recv() => break,
            packet = merged_stream.next() => {
                let packet = packet.context("device packet streams ended while recording")?;
                let timestamp = std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .context("cannot record packets before the Unix epoch")?
                    .as_millis();
                packets.push((timestamp, packet));
            }
        }
    }
    Ok(packets)
}

async fn run(output_path: &std::path::Path) -> Result<()> {
    let devices = get_devices().await?;
    if devices.len() > 1 {
        eprintln!("Multiple devices found, using first one");
    }
    let device_info = devices
        .into_iter()
        .next()
        .context("No device in USB mode found, please connect one")?;

    eprintln!("Connecting to device");
    let device = VmDevice::connect_usb(device_info)
        .await
        .context("failed to connect to device")?;
    eprintln!("Connected");

    let general_config = device
        .read_all_config()
        .await
        .context("failed to read config")?;
    eprintln!("Successfully read config");

    let (snd, mut stdin_line) = mpsc::channel(STDIN_EVENT_CAPACITY);
    std::thread::spawn(move || stdin_thread(snd));
    eprintln!("Press enter to start recording");

    stdin_line
        .recv()
        .await
        .context("stdin closed before recording started")?;
    eprintln!("Press enter to stop recording");

    let packets = record_packets(&device, &mut stdin_line).await?;
    let bytes =
        encode_recording(&general_config, &packets).context("failed to encode recording")?;

    eprintln!("Saving to {}", output_path.display());
    std::fs::write(output_path, &bytes)
        .with_context(|| format!("failed to write {}", output_path.display()))
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args.output).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}
