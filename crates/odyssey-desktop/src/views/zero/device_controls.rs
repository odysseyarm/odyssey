//! One device's row in the sidebar: what to zero on, and the zero and shot
//! delay controls.

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::Device;

use super::device_signals::DeviceSignals;
use super::save_zero_button::SaveZeroButton;
use super::shot_delay_control::ShotDelayControl;
use crate::hub::HubContext;

const BUTTON: &str = "btn-secondary";
const DANGER_BUTTON: &str = "btn-secondary-danger";

#[component]
pub(super) fn DeviceControls(
    hub: Signal<HubContext>,
    slot: usize,
    device: Device,
    signals: DeviceSignals,
) -> Element {
    let firing = (signals.shooting)();
    let zeroing_on_key = (signals.zeroing_on_key)();
    let label = crate::views::device_label(&device);
    rsx! {
        li {
            class: "flex flex-col gap-2",
            span {
                class: "text-gray-900 dark:text-white",
                {label},
            }
            ul { class: "flex flex-col gap-2",
                li { class: "flex items-center gap-3 flex-wrap",
                    if firing {
                        button {
                            class: DANGER_BUTTON,
                            onclick: move |_| signals.set_shooting(false),
                            "Cancel"
                        }
                    } else {
                        button {
                            class: BUTTON,
                            onclick: move |_| signals.set_shooting(true),
                            "Zero on shot"
                        }
                    }
                    button {
                        class: if zeroing_on_key { DANGER_BUTTON } else { BUTTON },
                        onclick: move |_| signals.toggle_zeroing_on_key(),
                        if zeroing_on_key { "Cancel [Z]" } else { "Zero on [Z]" }
                    }
                    button {
                        class: BUTTON,
                        onclick: {
                            let device = device.clone();
                            move |_| reset_zero(hub, device.clone())
                        },
                        "Reset Zero"
                    }
                    button {
                        class: BUTTON,
                        onclick: {
                            let device = device.clone();
                            move |_| clear_zero(hub, device.clone())
                        },
                        "Clear Zero"
                    }
                    SaveZeroButton { hub, device: device.clone(), signals }
                }
                ShotDelayControl { hub, slot, device: device.clone(), signals }
            }
        }
    }
}

async fn reset_zero(hub: Signal<HubContext>, device: Device) {
    let mut client = hub.peek().client.peek().clone();
    match client.reset_zero(device.clone()).await {
        Ok(()) => tracing::info!("Cleared zero for {:x?}", device.uuid),
        Err(e) => tracing::error!("Failed to reset zero {:x?}: {}", device.uuid, e),
    }
}

async fn clear_zero(hub: Signal<HubContext>, device: Device) {
    let mut client = hub.peek().client.peek().clone();
    match client.clear_zero(device.clone()).await {
        Ok(()) => tracing::info!("Cleared zero for {:x?}", device.uuid),
        Err(e) => tracing::error!("Failed to clear zero {:x?}: {}", device.uuid, e),
    }
}
