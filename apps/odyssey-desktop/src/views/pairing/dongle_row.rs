//! A dongle, its pairing controls, and the devices it has bonded or sees.

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_client::client::DongleInfo;
use odyssey_hub_common::events::Event;
use tokio::sync::broadcast::error::RecvError;

use super::device_row::device_label;
use super::pair_buttons::PairButtons;
use super::pairing_state::{PAIRING_TIMEOUT_MS, PairingState, result_status};
use crate::hub::HubContext;
use crate::views::device_label::format_address;

/// The dongle control protocol that can list its bonds.
const BONDS_PROTOCOL_VERSION: [u16; 3] = [0, 1, 1];
/// What a dongle reports before it has told us its firmware version.
const UNKNOWN_FIRMWARE_VERSION: [u16; 3] = [0, 0, 0];

const DOT_CONNECTED: &str = "inline-block w-2 h-2 rounded-full bg-green-500";
const DOT_DISCONNECTED: &str = "inline-block w-2 h-2 rounded-full bg-red-500";

async fn start(hub: Signal<HubContext>, dongle_id: String, state: PairingState) {
    state.started();
    let mut client = hub.peek().client.peek().clone();
    if let Err(e) = client
        .start_dongle_pairing(dongle_id, PAIRING_TIMEOUT_MS)
        .await
    {
        tracing::error!("Failed to start dongle pairing: {e}");
        state.start_failed(e);
    }
}

async fn cancel(hub: Signal<HubContext>, dongle_id: String, state: PairingState) {
    let mut client = hub.peek().client.peek().clone();
    match client.cancel_dongle_pairing(dongle_id).await {
        Ok(()) => state.cancelled(),
        Err(e) => {
            tracing::error!("Failed to cancel dongle pairing: {e}");
            state.error(e);
        }
    }
}

async fn clear(hub: Signal<HubContext>, dongle_id: String, state: PairingState) {
    let mut client = hub.peek().client.peek().clone();
    // Cancel pairing first if in progress
    if state.is_pairing() {
        if let Err(e) = client.cancel_dongle_pairing(dongle_id.clone()).await {
            tracing::error!("Failed to cancel dongle pairing before clearing bonds: {e}");
            state.error(e);
            return;
        }
        state.stopped();
    }
    match client.clear_dongle_bonds(dongle_id).await {
        Ok(()) => state.bonds_cleared("Bonds cleared"),
        Err(e) => {
            tracing::error!("Failed to clear dongle bonds: {e}");
            state.error(e);
        }
    }
}

/// Shows the result of this dongle's pairings. Pairing results arrive once,
/// so they come over their own channel.
fn use_pairing_results(hub: Signal<HubContext>, dongle_id: String, state: PairingState) {
    use_future(move || {
        let mut results = hub.peek().subscribe_pairing();
        let dongle_id = dongle_id.clone();
        async move {
            loop {
                let event = match results.recv().await {
                    Ok(event) => event,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                };
                let Event::DonglePairingResult {
                    dongle_id: event_id,
                    success,
                    paired_address,
                    paired_name,
                    error,
                } = &event
                else {
                    continue;
                };
                if *event_id != dongle_id {
                    continue;
                }
                state.finished(result_status(*success, paired_address, paired_name, error));
            }
        }
    });
}

#[component]
pub(super) fn DongleRow(hub: Signal<HubContext>, dongle: DongleInfo) -> Element {
    let state = PairingState::use_new();
    use_pairing_results(hub, dongle.id.clone(), state);

    let firmware = &dongle.firmware_version;
    let firmware_suffix = if *firmware == UNKNOWN_FIRMWARE_VERSION {
        String::new()
    } else {
        format!(" (fw {}.{}.{})", firmware[0], firmware[1], firmware[2])
    };
    // Dongles that can list their bonds show those; older ones only show who
    // is connected right now.
    let lists_bonds = dongle.control_protocol_version >= BONDS_PROTOCOL_VERSION;
    let (listed_addresses, list_label) = if lists_bonds {
        (&dongle.bonded_devices, "Bonds")
    } else {
        (&dongle.connected_devices, "Connected")
    };
    let devices = (hub.peek().devices)();

    rsx! {
        div {
            class: "p-3 bg-gray-50 dark:bg-gray-600 rounded-lg space-y-2",
            div {
                class: "flex items-center justify-between",
                div {
                    class: "flex items-center gap-2",
                    span {
                        class: "text-sm font-medium text-gray-700 dark:text-gray-200",
                        "Dongle"
                    }
                    span {
                        class: "text-xs font-mono text-gray-500 dark:text-gray-400",
                        "{dongle.id}{firmware_suffix}"
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
                        clear_label: "Clear Bonds",
                        on_pair: {
                            let id = dongle.id.clone();
                            move |()| state.run_command(start(hub, id.clone(), state))
                        },
                        on_cancel: {
                            let id = dongle.id.clone();
                            move |()| state.run_command(cancel(hub, id.clone(), state))
                        },
                        on_clear: {
                            let id = dongle.id.clone();
                            move |()| state.run_command(clear(hub, id.clone(), state))
                        },
                    }
                }
            }
            if !listed_addresses.is_empty() {
                div {
                    class: "ml-4 space-y-1",
                    div {
                        class: "text-[11px] uppercase tracking-wide text-gray-500 dark:text-gray-400",
                        {list_label}
                    }
                    for address in listed_addresses {
                        div {
                            class: "flex items-center gap-2",
                            span {
                                class: if dongle.connected_devices.contains(address) { DOT_CONNECTED } else { DOT_DISCONNECTED },
                            }
                            span {
                                class: "text-xs font-mono text-gray-600 dark:text-gray-300",
                                {devices
                                    .iter()
                                    .find(|(_, device)| device.uuid == *address)
                                    .map_or_else(|| format_address(address), |(_, device)| device_label(device))}
                            }
                        }
                    }
                }
            }
        }
    }
}
