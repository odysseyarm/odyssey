//! Keeps the per-device zeroing state in step with the devices that are
//! connected.

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::Device;

use super::device_signals::SignalsByDevice;
use crate::hub::HubContext;

/// When the set of connected devices changes, drops the state of the ones that
/// left and loads the stored shot delay of every device.
pub(super) fn use_device_signal_sync(
    hub: Signal<HubContext>,
    device_signals: Signal<SignalsByDevice>,
    devices: &[Device],
) {
    let mut last_snapshot = use_signal(Vec::<[u8; 6]>::new);
    let devices = devices.to_vec();
    let snapshot: Vec<[u8; 6]> = devices.iter().map(|device| device.uuid).collect();

    use_effect(move || {
        if last_snapshot.peek().as_slice() == snapshot.as_slice() {
            return;
        }
        last_snapshot.set(snapshot.clone());

        let devices = devices.clone();
        let present = snapshot.clone();
        spawn(async move {
            let mut device_signals = device_signals;
            device_signals
                .write()
                .retain(|uuid, _| present.contains(uuid));

            for device in devices {
                spawn(load_shot_delay(hub, device_signals, device));
            }
        });
    });
}

async fn load_shot_delay(
    hub: Signal<HubContext>,
    mut device_signals: Signal<SignalsByDevice>,
    device: Device,
) {
    let mut client = hub.peek().client.peek().clone();
    match client.get_shot_delay(device.clone()).await {
        Ok(delay) => {
            device_signals
                .write()
                .entry(device.uuid)
                .or_default()
                .shot_delay
                .set(delay);
        }
        Err(e) => tracing::error!("Failed to get shot delay: {e}"),
    }
}
