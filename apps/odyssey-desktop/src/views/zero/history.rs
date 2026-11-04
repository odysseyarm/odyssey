//! A short history of tracking events per device, so a zero can use the sample
//! from the moment of the shot instead of the one from the moment of the click.

use std::collections::HashMap;

use dioxus::prelude::*;
use odyssey_hub_client::tracking_history::TrackingHistory;
use odyssey_hub_common::events::TrackingEvent;
use tokio::sync::broadcast::error::RecvError;

use crate::hub::HubContext;

/// Samples kept per device, about 2 seconds at 100 Hz.
const TRACKING_HISTORY_CAP: usize = 200;

/// Milliseconds to the microseconds of the device's timestamps.
const MICROS_PER_MILLI: u32 = 1000;

/// Recent tracking events, by device uuid.
pub(super) type HistoryByDevice = HashMap<[u8; 6], TrackingHistory>;

/// Buffers every tracking event the hub broadcasts, per device. Subscribes
/// through the broadcast channel, which works across VirtualDom windows.
pub(super) fn use_tracking_history(hub: Signal<HubContext>) -> Signal<HistoryByDevice> {
    let mut history = use_signal(HistoryByDevice::new);
    use_future(move || {
        let mut receiver = hub.peek().subscribe_tracking();
        async move {
            loop {
                match receiver.recv().await {
                    Ok((device, event)) => {
                        history
                            .write()
                            .entry(device.uuid)
                            .or_insert_with(|| TrackingHistory::new(TRACKING_HISTORY_CAP))
                            .push(event);
                    }
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => break,
                }
            }
        }
    });
    history
}

/// The buffered sample closest to the moment of the shot.
///
/// `impact_timestamp` is the shot's own device timestamp. Without one (a zero
/// by key press), or with the `u32::MAX` sentinel that accessories report, the
/// shot is taken to be `shot_delay_ms` before the newest sample, which is at
/// most one sample interval stale.
pub(super) fn closest_sample(
    history: &HistoryByDevice,
    uuid: &[u8; 6],
    shot_delay_ms: u16,
    impact_timestamp: Option<u32>,
) -> Option<TrackingEvent> {
    let device_history = history.get(uuid)?;
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
        let sample = closest_sample(&history, &[1; 6], 20, None).expect("a sample");
        assert_eq!(sample.timestamp, 1_010_000);
    }

    #[test]
    fn an_impact_looks_back_from_its_own_timestamp() {
        let history = history_of(&[1_000_000, 1_010_000, 1_020_000, 1_030_000]);
        let sample = closest_sample(&history, &[1; 6], 10, Some(1_025_000)).expect("a sample");
        assert_eq!(sample.timestamp, 1_010_000);
    }

    #[test]
    fn the_accessory_sentinel_falls_back_to_the_newest_sample() {
        let history = history_of(&[1_000_000, 1_030_000]);
        let sample = closest_sample(&history, &[1; 6], 0, Some(u32::MAX)).expect("a sample");
        assert_eq!(sample.timestamp, 1_030_000);
    }

    #[test]
    fn a_device_without_history_has_no_sample() {
        let history = history_of(&[1_000_000]);
        assert!(closest_sample(&history, &[2; 6], 0, None).is_none());
    }
}
