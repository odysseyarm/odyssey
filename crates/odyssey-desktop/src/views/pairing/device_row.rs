//! A device that pairs with a controller, and its pairing controls.

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event};
use tokio::sync::broadcast::error::RecvError;

use super::pair_buttons::PairButtons;
use super::pairing_state::{PAIRING_TIMEOUT_MS, PairingState, result_status};
use crate::hub::HubContext;
use crate::views::device_label;

/// The UI-facing reason a call could not be made without a hub connection.
const NOT_CONNECTED: &str = "not connected to the hub";

async fn start(hub: Signal<HubContext>, device: Device, state: PairingState) {
    state.started();
    let Some(client) = hub.peek().client() else {
        tracing::warn!("Failed to start pairing: {NOT_CONNECTED}");
        state.start_failed(NOT_CONNECTED);
        return;
    };
    if let Err(e) = client
        .device(device)
        .start_pairing(PAIRING_TIMEOUT_MS)
        .await
    {
        tracing::error!("Failed to start pairing: {e}");
        state.start_failed(e);
    }
}

async fn cancel(hub: Signal<HubContext>, device: Device, state: PairingState) {
    let Some(client) = hub.peek().client() else {
        tracing::warn!("Failed to cancel pairing: {NOT_CONNECTED}");
        state.error(NOT_CONNECTED);
        return;
    };
    match client.device(device).cancel_pairing().await {
        Ok(()) => state.cancelled(),
        Err(e) => {
            tracing::error!("Failed to cancel pairing: {e}");
            state.error(e);
        }
    }
}

async fn clear(hub: Signal<HubContext>, device: Device, state: PairingState) {
    let Some(client) = hub.peek().client() else {
        tracing::warn!("Failed to clear bond: {NOT_CONNECTED}");
        state.error(NOT_CONNECTED);
        return;
    };
    let client = client.device(device);
    if state.is_pairing() {
        if let Err(e) = client.cancel_pairing().await {
            tracing::error!("Failed to cancel pairing before clearing bond: {e}");
            state.error(e);
            return;
        }
        state.stopped();
    }
    match client.clear_bond().await {
        Ok(()) => state.bonds_cleared("Bond cleared"),
        Err(e) => {
            tracing::error!("Failed to clear bond: {e}");
            state.error(e);
        }
    }
}

/// Shows the result of this device's pairings, which the hub reports once.
fn use_pairing_results(hub: Signal<HubContext>, device_uuid: [u8; 6], state: PairingState) {
    use_future(move || {
        let mut results = hub.peek().subscribe_pairing();
        async move {
            loop {
                let event = match results.recv().await {
                    Ok(event) => event,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                };
                let Event::DeviceEvent(DeviceEvent(
                    device,
                    DeviceEventKind::PairingResult {
                        success,
                        paired_address,
                        paired_name,
                        error,
                    },
                )) = &event
                else {
                    continue;
                };
                if device.uuid != device_uuid {
                    continue;
                }
                state.finished(result_status(*success, paired_address, paired_name, error));
            }
        }
    });
}

#[component]
pub(super) fn DeviceRow(hub: Signal<HubContext>, device: Device) -> Element {
    let state = PairingState::use_new();
    use_pairing_results(hub, device.uuid, state);

    rsx! {
        div {
            class: "flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-600 rounded-lg",
            div {
                class: "flex items-center gap-2",
                span {
                    class: "text-sm font-mono text-gray-700 dark:text-gray-200",
                    "{device_label(&device)}"
                }
                if !state.status.read().is_empty() {
                    span {
                        class: "text-xs text-gray-500 dark:text-gray-400 italic",
                        "{state.status}"
                    }
                }
            }
            div {
                class: "flex items-center gap-1",
                PairButtons {
                    pairing: *state.pairing.read(),
                    command_pending: (state.command_pending)(),
                    clear_label: "Clear Bond",
                    clear_prompt: "Forget this device's bond?",
                    on_pair: {
                        let device = device.clone();
                        move |()| state.run_command(start(hub, device.clone(), state))
                    },
                    on_cancel: {
                        let device = device.clone();
                        move |()| state.run_command(cancel(hub, device.clone(), state))
                    },
                    on_clear: move |()| state.run_command(clear(hub, device.clone(), state)),
                }
            }
        }
    }
}
