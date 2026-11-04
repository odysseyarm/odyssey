//! A device that pairs with a controller, and its pairing controls.

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event};
use tokio::sync::broadcast::error::RecvError;

use super::pair_buttons::PairButtons;
use super::pairing_state::{PAIRING_TIMEOUT_MS, PairingState, result_status};
use crate::hub::HubContext;
use crate::views::device_label::device_type;

/// Pairing labels use the same uppercase address as the dongle's bond list.
pub(super) fn device_label(device: &Device) -> String {
    let addr = crate::views::device_label::format_address(&device.uuid);
    let ty = device_type(device);
    let name = device.name();
    if name.is_empty() {
        format!("{ty} ({addr})")
    } else {
        format!("{ty}: {name} ({addr})")
    }
}

async fn start(hub: Signal<HubContext>, device: Device, state: PairingState) {
    state.started();
    let mut client = hub.peek().client.peek().clone();
    if let Err(e) = client.start_pairing(device, PAIRING_TIMEOUT_MS).await {
        tracing::error!("Failed to start pairing: {e}");
        state.start_failed(e);
    }
}

async fn cancel(hub: Signal<HubContext>, device: Device, state: PairingState) {
    let mut client = hub.peek().client.peek().clone();
    match client.cancel_pairing(device).await {
        Ok(()) => state.cancelled(),
        Err(e) => {
            tracing::error!("Failed to cancel pairing: {e}");
            state.error(e);
        }
    }
}

async fn clear(hub: Signal<HubContext>, device: Device, state: PairingState) {
    let mut client = hub.peek().client.peek().clone();
    if state.is_pairing() {
        if let Err(e) = client.cancel_pairing(device.clone()).await {
            tracing::error!("Failed to cancel pairing before clearing bond: {e}");
            state.error(e);
            return;
        }
        state.stopped();
    }
    match client.clear_bond(device).await {
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
