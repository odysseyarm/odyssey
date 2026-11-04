//! Records the device's raw packet streams to a file that `odyssey-playback`
//! can read (see `odyssey_vision_tool::recording` for the layout).

#![expect(
    clippy::print_stderr,
    reason = "command-line tool: stderr is its interface"
)]

use std::process::ExitCode;
use std::time::UNIX_EPOCH;

use anyhow::{Context as _, Result};
use nusb::{DeviceInfo, MaybeFuture as _};
use odyssey_protocol::control::device::TransportMode;
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::{PacketData, PacketType};
use odyssey_vision_tool::recording::encode_recording;
use tokio::sync::mpsc::{self, Sender};
use tokio_stream::StreamExt as _;

/// Nordic Semiconductor's USB vendor id.
const NORDIC_VENDOR_ID: u16 = 0x1915;
/// Product ids of the devices that can be recorded.
const RECORDABLE_PRODUCT_IDS: [u16; 2] = [0x520F, 0x5210];

/// Capacity of the channel carrying "Enter was pressed" events.
const STDIN_EVENT_CAPACITY: usize = 50;

async fn get_devices() -> Vec<DeviceInfo> {
    let devices = match nusb::list_devices().wait() {
        Ok(devs) => devs,
        Err(e) => {
            eprintln!("Failed to list USB devices: {e}");
            return vec![];
        }
    };

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

    filtered
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
    // One merged stream so a single select! arm waits for any of the three.
    let mut merged_stream = accel_stream.merge(mot_stream).merge(comb_stream);

    let mut packets = vec![];
    loop {
        tokio::select! {
            _ = stdin_line.recv() => break,
            packet = merged_stream.next() => {
                let Some(packet) = packet else { break };
                let timestamp = std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock is after the Unix epoch")
                    .as_millis();
                packets.push((timestamp, packet));
            }
        }
    }
    Ok(packets)
}

async fn run(output_path: &str, port_arg: Option<&str>) -> Result<()> {
    if port_arg.is_some() {
        eprintln!("Port argument is no longer supported, using first available device");
    }
    let devices = get_devices().await;
    if port_arg.is_none() && devices.len() > 1 {
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

    stdin_line.recv().await;
    eprintln!("Press enter to stop recording");

    let packets = record_packets(&device, &mut stdin_line).await?;
    let bytes =
        encode_recording(&general_config, &packets).context("failed to encode recording")?;

    eprintln!("Saving to {output_path}");
    std::fs::write(output_path, &bytes).with_context(|| format!("failed to write {output_path}"))
}

#[tokio::main]
async fn main() -> ExitCode {
    let Some(output_path) = std::env::args().nth(1) else {
        eprintln!("No output path");
        return ExitCode::FAILURE;
    };
    let port_arg = std::env::args().nth(2);

    match run(&output_path, port_arg.as_deref()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}
