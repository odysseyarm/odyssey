//! Sending a zero to the hub, from a key press or from a detected shot.

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::TrackingEvent;
use tokio::sync::broadcast::error::RecvError;

use super::ZERO_TARGET;
use super::device_signals::SignalsByDevice;
use super::history::{HistoryByDevice, closest_sample};
use crate::hub::HubContext;

/// The translation sent with every zero request, in meters.
const ZERO_TRANSLATION_M: nalgebra::Vector3<f32> = nalgebra::Vector3::new(0., -0.0635, 0.);

/// The key that zeroes every device waiting for it.
const ZERO_KEY: &str = "z";
/// The key that gives up waiting without zeroing.
const CANCEL_KEY: &str = "Escape";

/// Zeroes `device` against the screen center. `sample` is the tracking event
/// from the moment of the shot, when one is known.
pub(super) async fn zero_device(
    hub: Signal<HubContext>,
    device: Device,
    sample: Option<TrackingEvent>,
) {
    let Some(client) = hub.peek().client() else {
        tracing::warn!(
            "Failed to zero device {:x?}: not connected to the hub",
            device.uuid
        );
        return;
    };
    if let Err(e) = client
        .device(device.clone())
        .zero(ZERO_TRANSLATION_M.into(), ZERO_TARGET.into(), sample)
        .await
    {
        tracing::error!("Failed to zero device {:x?}: {}", device.uuid, e);
    }
}

/// Handles a key press: Z zeroes every device that is waiting for it, Escape
/// stops them waiting.
pub(super) fn zero_on_key(
    hub: Signal<HubContext>,
    device_signals: Signal<SignalsByDevice>,
    tracking_history: Signal<HistoryByDevice>,
    key: String,
) {
    if key != ZERO_KEY && key != CANCEL_KEY {
        return;
    }
    let devices: Vec<Device> = (hub.peek().devices)()
        .into_iter()
        .map(|(_, device)| device)
        .collect();
    for device in devices {
        let Some(signals) = device_signals.peek().get(&device.uuid).copied() else {
            continue;
        };
        if !*signals.zeroing_on_key.peek() {
            continue;
        }
        signals.set_zeroing_on_key(false);
        if key == CANCEL_KEY {
            continue;
        }
        let sample = closest_sample(
            &tracking_history.peek(),
            &device.uuid,
            signals.shot_delay_ms(),
            None,
        );
        spawn(zero_device(hub, device, sample));
    }
}

/// Zeroes on every shot of a device that is waiting for one. Impact events
/// come over a broadcast channel, so this works across VirtualDom windows.
pub(super) fn use_zero_on_impact(
    hub: Signal<HubContext>,
    device_signals: Signal<SignalsByDevice>,
    tracking_history: Signal<HistoryByDevice>,
) {
    use_future(move || {
        let mut receiver = hub.peek().subscribe_impact();
        async move {
            loop {
                match receiver.recv().await {
                    Ok((device, impact)) => {
                        zero_if_armed(
                            hub,
                            device_signals,
                            tracking_history,
                            device,
                            impact.timestamp,
                        );
                    }
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => break,
                }
            }
        }
    });
}

fn zero_if_armed(
    hub: Signal<HubContext>,
    device_signals: Signal<SignalsByDevice>,
    tracking_history: Signal<HistoryByDevice>,
    device: Device,
    impact_timestamp: u32,
) {
    let Some(signals) = device_signals.peek().get(&device.uuid).copied() else {
        tracing::warn!("ImpactEvent: no zeroing state for {:02x?}", device.uuid);
        return;
    };
    let shooting = *signals.shooting.peek();
    tracing::info!(
        "ImpactEvent for {:02x?}: shooting={}",
        device.uuid,
        shooting
    );
    if !shooting {
        return;
    }
    let sample = closest_sample(
        &tracking_history.peek(),
        &device.uuid,
        signals.shot_delay_ms(),
        Some(impact_timestamp),
    );
    signals.set_shooting(false);
    spawn(zero_device(hub, device, sample));
}
