//! A short history of tracking events per device, so a zero can use the sample
//! from the moment of the shot instead of the one from the moment of the click.

use std::collections::HashMap;

use futures::StreamExt as _;
use gpui::{Context, Task};
use odyssey_hub_client::client::Client;
use odyssey_hub_client::tracking_history::TrackingHistory;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event, TrackingEvent};
use tokio::sync::{mpsc, watch};

use super::ZeroView;
use crate::runtime;

/// Samples kept per device, about 2 seconds at 100 Hz.
const TRACKING_HISTORY_CAP: usize = 200;

/// Milliseconds to the microseconds of the device's timestamps.
const MICROS_PER_MILLI: u32 = 1000;

/// Recent tracking events, by device uuid.
pub(super) type HistoryByDevice = HashMap<[u8; 6], TrackingHistory>;

fn record(history: &mut HistoryByDevice, uuid: [u8; 6], event: TrackingEvent) {
    history
        .entry(uuid)
        .or_insert_with(|| TrackingHistory::new(TRACKING_HISTORY_CAP))
        .push(event);
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn stream_tracking(
    updates: mpsc::Sender<(Device, TrackingEvent)>,
    mut screen: watch::Receiver<Option<u8>>,
) {
    loop {
        let selected = *screen.borrow_and_update();
        let Some(selected) = selected else {
            if screen.changed().await.is_err() {
                return;
            }
            continue;
        };
        let result = tokio::select! {
            changed = screen.changed() => {
                if changed.is_err() {
                    return;
                }
                continue;
            }
            result = async {
                let client = Client::connect().await?;
                let mut events = client.subscribe_events_for_screen(selected).await?;
                while let Some(event) = events.next().await {
                    if let Event::DeviceEvent(DeviceEvent(
                        device,
                        DeviceEventKind::TrackingEvent(tracking),
                    )) = event?
                        && tracking.screen_id == u32::from(selected)
                        && updates.send((device, tracking)).await.is_err()
                    {
                        return Ok::<_, eros::ErrorUnion>(());
                    }
                }
                Ok(())
            } => result,
        };
        if updates.is_closed() {
            return;
        }
        if let Err(error) = result {
            tracing::warn!("Zero tracking stream disconnected: {error}");
        }
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
            changed = screen.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

/// Window-owned RPC stream keeps markers active only while Zero is open.
pub(super) fn follow_tracking(
    screen: watch::Receiver<Option<u8>>,
    cx: &mut Context<ZeroView>,
) -> Task<()> {
    let (updates, mut receiver) = mpsc::channel(128);
    let stream = AbortOnDrop(runtime::spawn(stream_tracking(updates, screen)));
    cx.spawn(async move |this, cx| {
        let _stream = stream;
        while let Some((device, event)) = receiver.recv().await {
            let recorded = this.update(cx, |zero, cx| {
                if zero
                    .devices
                    .iter()
                    .any(|known| known.uuid == device.uuid && known.events_connected)
                    && zero
                        .tracking_screen
                        .borrow()
                        .is_some_and(|screen| event.screen_id == u32::from(screen))
                {
                    record(&mut zero.history, device.uuid, event);
                    zero.tracking
                        .insert(device.uuid, (event, std::time::Instant::now()));
                    if zero.selected_device == Some(device.uuid) {
                        zero.refresh_readiness(cx);
                        cx.notify();
                    }
                }
            });
            if recorded.is_err() {
                break;
            }
        }
    })
}

/// The buffered sample closest to the moment of the shot.
///
/// `impact_timestamp` is the shot's own device timestamp. Without one (a zero
/// by key press), or with the `u32::MAX` sentinel that accessories report, the
/// shot is taken to be `shot_delay_ms` before the newest sample, which is at
/// most one sample interval stale.
pub(super) fn closest_sample(
    history: &HistoryByDevice,
    uuid: [u8; 6],
    shot_delay_ms: u16,
    impact_timestamp: Option<u32>,
) -> Option<TrackingEvent> {
    let device_history = history.get(&uuid)?;
    let shot_delay_us = u32::from(shot_delay_ms) * MICROS_PER_MILLI;
    let lookup_timestamp = match impact_timestamp {
        Some(timestamp) if timestamp != u32::MAX => timestamp.wrapping_sub(shot_delay_us),
        _ => device_history.latest().map_or(u32::MAX, |latest| {
            latest.timestamp.wrapping_sub(shot_delay_us)
        }),
    };
    device_history.get_closest(lookup_timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_at(timestamp: u32) -> TrackingEvent {
        TrackingEvent {
            timestamp,
            aimpoint: nalgebra::Vector2::zeros(),
            pose: odyssey_hub_common::events::Pose {
                rotation: nalgebra::Matrix3::identity(),
                translation: nalgebra::Matrix3x1::zeros(),
            },
            distance: 0.0,
            screen_id: 0,
        }
    }

    fn history_of(timestamps: &[u32]) -> HistoryByDevice {
        let mut ring = TrackingHistory::new(16);
        for &timestamp in timestamps {
            ring.push(event_at(timestamp));
        }
        HashMap::from([([1; 6], ring)])
    }

    #[test]
    fn a_key_press_looks_back_from_the_newest_sample() {
        let history = history_of(&[1_000_000, 1_010_000, 1_020_000, 1_030_000]);
        // 20 ms before the newest sample lands on the second one.
        let sample = closest_sample(&history, [1; 6], 20, None).expect("a sample");
        assert_eq!(sample.timestamp, 1_010_000);
    }

    #[test]
    fn an_impact_looks_back_from_its_own_timestamp() {
        let history = history_of(&[1_000_000, 1_010_000, 1_020_000, 1_030_000]);
        let sample = closest_sample(&history, [1; 6], 10, Some(1_025_000)).expect("a sample");
        assert_eq!(sample.timestamp, 1_010_000);
    }

    #[test]
    fn the_accessory_sentinel_falls_back_to_the_newest_sample() {
        let history = history_of(&[1_000_000, 1_030_000]);
        let sample = closest_sample(&history, [1; 6], 0, Some(u32::MAX)).expect("a sample");
        assert_eq!(sample.timestamp, 1_030_000);
    }

    #[test]
    fn a_device_without_history_has_no_sample() {
        let history = history_of(&[1_000_000]);
        assert!(closest_sample(&history, [2; 6], 0, None).is_none());
    }
}
