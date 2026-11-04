//! Keeps the per-device zeroing state in step with the devices that are
//! connected.

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::Device;

use super::device_signals::{DeviceSignals, SignalsByDevice};
use crate::hub::HubContext;

/// Drops state when devices leave and loads the stored shot delay only for
/// newly connected devices. Late replies never recreate removed state or
/// overwrite a delay the user has already edited.
pub(super) fn use_device_signal_sync(
    hub: Signal<HubContext>,
    device_signals: Signal<SignalsByDevice>,
) {
    let mut last_snapshot = use_signal(Vec::<[u8; 6]>::new);

    use_effect(move || {
        let devices = (hub().devices)();
        let snapshot: Vec<_> = devices.iter().map(|(_, device)| device.uuid).collect();
        if last_snapshot.peek().as_slice() == snapshot.as_slice() {
            return;
        }
        let previous = last_snapshot.peek().clone();
        last_snapshot.set(snapshot.clone());
        let mut device_signals = device_signals;
        device_signals
            .write()
            .retain(|uuid, _| snapshot.contains(uuid));

        for (_, device) in devices {
            if previous.contains(&device.uuid) {
                continue;
            }
            let signals = *device_signals.write().entry(device.uuid).or_default();
            let revision = signals.shot_delay_revision();
            spawn(load_shot_delay(
                hub,
                device_signals,
                device,
                signals,
                revision,
            ));
        }
    });
}

async fn load_shot_delay(
    hub: Signal<HubContext>,
    device_signals: Signal<SignalsByDevice>,
    device: Device,
    signals: DeviceSignals,
    revision: u64,
) {
    let Some(client) = hub.peek().client() else {
        tracing::warn!("Failed to get shot delay: not connected to the hub");
        return;
    };
    match client.device(device.clone()).get_shot_delay().await {
        Ok(delay) => {
            let still_present = device_signals
                .peek()
                .get(&device.uuid)
                .is_some_and(|current| *current == signals);
            if still_present && signals.shot_delay_revision() == revision {
                signals.set_shot_delay_ms(delay);
            }
        }
        Err(e) => tracing::error!("Failed to get shot delay: {e}"),
    }
}
