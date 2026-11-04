//! The button that stores the current zero on the device.

use std::time::Duration;

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::Device;

use super::device_signals::DeviceSignals;
use crate::hub::HubContext;

/// How long "Saved" stays on the button before it goes back to idle.
const SAVED_SHOWN_FOR: Duration = Duration::from_secs(2);
/// How long an error stays on the button, long enough to read.
const ERROR_SHOWN_FOR: Duration = Duration::from_secs(4);

#[component]
pub(super) fn SaveZeroButton(
    hub: Signal<HubContext>,
    device: Device,
    signals: DeviceSignals,
) -> Element {
    let (class, label) = match (signals.save_zero_status)() {
        None => ("btn-secondary", "Save Zero".to_string()),
        Some(Ok(())) => ("btn-secondary-success", "Saved".to_string()),
        Some(Err(e)) => ("btn-secondary-danger", format!("Error: {e}")),
    };
    rsx! {
        button {
            class,
            onclick: move |_| save_zero(hub, device.clone(), signals),
            {label}
        }
    }
}

async fn save_zero(hub: Signal<HubContext>, device: Device, signals: DeviceSignals) {
    let mut client = hub.peek().client.peek().clone();
    match client.save_zero(device.clone()).await {
        Ok(()) => {
            tracing::info!("Saved zero for {:x?}", device.uuid);
            signals.set_save_status(Some(Ok(())));
            reset_status_after(signals, SAVED_SHOWN_FOR);
        }
        Err(e) => {
            let message = e.to_string();
            tracing::error!("Failed to save zero {:x?}: {}", device.uuid, message);
            signals.set_save_status(Some(Err(message)));
            reset_status_after(signals, ERROR_SHOWN_FOR);
        }
    }
}

fn reset_status_after(signals: DeviceSignals, delay: Duration) {
    spawn(async move {
        tokio::time::sleep(delay).await;
        signals.set_save_status(None);
    });
}
