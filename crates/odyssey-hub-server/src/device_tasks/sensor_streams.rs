use std::time::Duration;

use futures::stream::{BoxStream, SelectAll, Stream, StreamExt as _};
use odyssey_usb::Result;
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::{
    AccelReport, BatteryReport, CombinedMarkersReport, ImpactReport, PocMarkersReport,
};
use tokio::time::{Instant, Sleep, sleep};
use tracing::{debug, info, warn};

/// First events-protocol version that carries the battery stream.
const BATTERY_STREAM_PROTOCOL: [u16; 3] = [0, 1, 1];

/// A stream that has stopped is retried this often.
pub(super) const STREAMS_RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// After the last register access of a batch, wait this long before restarting the streams.
const STREAMS_RESUME_DEBOUNCE: Duration = Duration::from_millis(100);

/// Stands in for "never fires" while a timer is disarmed.
const DISARMED_TIMER: Duration = Duration::from_secs(3600);

pub(super) enum DeviceStreamEvent {
    CombinedMarkers(CombinedMarkersReport),
    PocMarkers(PocMarkersReport),
    Accel(AccelReport),
    Impact(ImpactReport),
    Battery(BatteryReport),
}

impl DeviceStreamEvent {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Accel(_) => "Accel",
            Self::Impact(_) => "Impact",
            Self::Battery(_) => "Battery",
            Self::CombinedMarkers(_) => "CombinedMarkers",
            Self::PocMarkers(_) => "PocMarkers",
        }
    }
}

/// The sensor streams of one unit's events device, merged into one tagged stream.
pub(super) struct SensorStreams {
    streams: SelectAll<BoxStream<'static, DeviceStreamEvent>>,
    /// Whether the merged stream should be polled. Cleared when the streams are
    /// deliberately stopped or have all ended, and set again by [`Self::start`].
    running: bool,
}

impl SensorStreams {
    pub(super) fn stopped() -> Self {
        Self {
            streams: SelectAll::new(),
            running: false,
        }
    }

    /// Start all sensor streams for a given events device. Streams that fail to start are
    /// logged and skipped, so the result can be empty (check [`Self::is_running`]).
    pub(super) async fn start(
        events_device: &VmDevice,
        uuid: [u8; 6],
        protocol_version: [u16; 3],
    ) -> Self {
        info!(
            "start_sensor_streams for {uuid:02x?}: protocol_version={}.{}.{}",
            protocol_version[0], protocol_version[1], protocol_version[2]
        );
        let mut streams = SelectAll::new();
        push_stream(
            &mut streams,
            uuid,
            "combined markers",
            events_device.clone().stream_combined_markers().await,
            DeviceStreamEvent::CombinedMarkers,
        );
        push_stream(
            &mut streams,
            uuid,
            "PoC markers",
            events_device.clone().stream_poc_markers().await,
            DeviceStreamEvent::PocMarkers,
        );
        push_stream(
            &mut streams,
            uuid,
            "accelerometer",
            events_device.clone().stream_accel().await,
            DeviceStreamEvent::Accel,
        );
        push_stream(
            &mut streams,
            uuid,
            "impact",
            events_device.clone().stream_impact().await,
            DeviceStreamEvent::Impact,
        );
        if protocol_version >= BATTERY_STREAM_PROTOCOL {
            push_stream(
                &mut streams,
                uuid,
                "battery",
                events_device.clone().stream_battery().await,
                DeviceStreamEvent::Battery,
            );
        } else {
            debug!(
                "Skipping battery stream for {uuid:02x?}: protocol version {}.{}.{} < 0.1.1",
                protocol_version[0], protocol_version[1], protocol_version[2]
            );
        }
        Self {
            running: !streams.is_empty(),
            streams,
        }
    }

    pub(super) fn is_running(&self) -> bool {
        self.running
    }

    /// Drop every stream and stop polling until the next [`Self::start`].
    pub(super) fn stop(&mut self) {
        self.streams = SelectAll::new();
        self.running = false;
    }

    pub(super) async fn next_event(&mut self) -> Option<DeviceStreamEvent> {
        self.streams.next().await
    }
}

fn push_stream<S, Report>(
    streams: &mut SelectAll<BoxStream<'static, DeviceStreamEvent>>,
    uuid: [u8; 6],
    name: &str,
    started: Result<S>,
    wrap: fn(Report) -> DeviceStreamEvent,
) where
    S: Stream<Item = Report> + Send + 'static,
    Report: 'static,
{
    match started {
        Ok(stream) => {
            info!("{name} stream started for {uuid:02x?}");
            streams.push(stream.map(wrap).boxed());
        }
        Err(e) => warn!("Failed to start {name} stream for {uuid:02x?}: {e}"),
    }
}

/// Timers that stop and restart a unit's sensor streams.
///
/// Register reads and writes pause the streams once per batch (the firmware cannot serve
/// them while its object loop holds the sensor mutex) and restart them after a debounce.
/// Streams that are not running are retried on a fixed interval.
pub(super) struct StreamRestartTimers {
    /// Streams were stopped for register access and have not been restarted yet.
    pub(super) paused: bool,
    pub(super) resume_sleep: std::pin::Pin<Box<Sleep>>,
    pub(super) resume_armed: bool,
    pub(super) retry_sleep: std::pin::Pin<Box<Sleep>>,
}

impl StreamRestartTimers {
    pub(super) fn new() -> Self {
        Self {
            paused: false,
            resume_sleep: Box::pin(sleep(DISARMED_TIMER)),
            resume_armed: false,
            retry_sleep: Box::pin(sleep(STREAMS_RETRY_INTERVAL)),
        }
    }

    /// Restart the debounce: streams come back once no register access arrives for a while.
    /// A no-op unless the streams are paused.
    pub(super) fn arm_resume(&mut self) {
        if self.paused {
            self.resume_sleep
                .as_mut()
                .reset(Instant::now() + STREAMS_RESUME_DEBOUNCE);
            self.resume_armed = true;
        }
    }

    pub(super) fn delay_retry(&mut self) {
        self.retry_sleep
            .as_mut()
            .reset(Instant::now() + STREAMS_RETRY_INTERVAL);
    }
}
