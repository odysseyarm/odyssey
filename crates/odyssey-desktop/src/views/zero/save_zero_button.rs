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
    let mut saving = use_signal(|| false);
    // Restart the timer whenever a new result replaces the previous one.
    // Independent spawned timers could clear a newer result too early.
    use_resource(move || async move {
        let delay = match (signals.save_zero_status)() {
            None => return,
            Some(Ok(())) => SAVED_SHOWN_FOR,
            Some(Err(_)) => ERROR_SHOWN_FOR,
        };
        tokio::time::sleep(delay).await;
        signals.set_save_status(None);
    });
    let (class, label) = match (signals.save_zero_status)() {
        None => ("btn btn-secondary btn-sm", "Save Zero".to_string()),
        Some(Ok(())) => (
            "btn btn-secondary btn-sm border-success/40 text-success-ink",
            "Saved".to_string(),
        ),
        Some(Err(e)) => (
            "btn btn-secondary btn-sm text-danger-ink hover:border-danger-ink",
            format!("Error: {e}"),
        ),
    };
    rsx! {
        button {
            class,
            disabled: saving(),
            onclick: move |_| {
                if *saving.peek() {
                    return;
                }
                saving.set(true);
                let device = device.clone();
                spawn(async move {
                    save_zero(hub, device, signals).await;
                    saving.set(false);
                });
            },
            {label}
        }
    }
}

async fn save_zero(hub: Signal<HubContext>, device: Device, signals: DeviceSignals) {
    let Some(client) = hub.peek().client() else {
        let message = "not connected to the hub".to_owned();
        tracing::warn!("Failed to save zero {:x?}: {}", device.uuid, message);
        signals.set_save_status(Some(Err(message)));
        return;
    };
    match client.device(device.clone()).save_zero().await {
        Ok(()) => {
            tracing::info!("Saved zero for {:x?}", device.uuid);
            signals.set_save_status(Some(Ok(())));
        }
        Err(e) => {
            let message = e.to_string();
            tracing::error!("Failed to save zero {:x?}: {}", device.uuid, message);
            signals.set_save_status(Some(Err(message)));
        }
    }
}
