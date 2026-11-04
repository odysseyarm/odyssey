use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::{Device, DeviceCapabilities};

use super::op_status::{OpStatus, StatusLine, rpc_failure};
use super::product_ids::supports_transport_switching;
use super::styles::{
    ACTION_PRIMARY_CLASS, ACTION_SECONDARY_CLASS, DISCLOSURE_SUMMARY_CLASS, FIELD_GRID_CLASS,
    HELP_TEXT_CLASS, LABEL_CLASS,
};
use crate::hub::HubContext;

fn mode_button_class(active: bool) -> &'static str {
    if active {
        ACTION_PRIMARY_CLASS
    } else {
        ACTION_SECONDARY_CLASS
    }
}

/// Asks the hub to switch the device's events transport and reports progress
/// through `status`; the device reboots, so success only means "requested".
async fn switch_transport_mode(
    hub: Signal<HubContext>,
    device: Device,
    usb_mode: bool,
    mut status: Signal<OpStatus>,
    mut user_pending_usb: Signal<Option<bool>>,
    mut switching: Signal<bool>,
) {
    if *switching.peek() {
        return;
    }
    switching.set(true);
    status.set(OpStatus::Busy("Switching...".into()));
    let hub_ctx = hub.peek().clone();
    let mut client = hub_ctx.client.peek().clone();
    match client.set_transport_mode(device, usb_mode).await {
        Ok(confirmed_usb) => {
            user_pending_usb.set(Some(confirmed_usb));
            let mode_str = if confirmed_usb { "USB" } else { "Bluetooth" };
            status.set(OpStatus::Busy(format!(
                "Switching to {mode_str}. The device restarts and reconnects on its own."
            )));
        }
        Err(e) => {
            tracing::error!("set_transport_mode failed: {e:#}");
            user_pending_usb.set(None);
            status.set(rpc_failure("switch the transport mode", &e));
        }
    }
    switching.set(false);
}

/// Shows a transport mode selector (BLE / USB) for ATS Lite / Lite1 devices.
/// Requires only CONTROL capability, so it is visible even when the device is
/// in BLE mode and the EVENTS bulk endpoint is not available.
#[component]
pub(crate) fn TransportModePanel(hub: Signal<HubContext>, device: Device) -> Element {
    let should_render = supports_transport_switching(device.product_id)
        && device.capabilities.contains(DeviceCapabilities::CONTROL);

    let mut status = use_signal(OpStatus::default);
    // The mode the user asked for, `None` when there is no override. Cleared once
    // the device prop shows that mode, which happens after the reboot.
    let mut user_pending_usb = use_signal(|| Option::<bool>::None);
    let switching = use_signal(|| false);

    // The displayed mode prefers the user's request while the reboot is pending,
    // and otherwise follows the device prop.
    let device_is_usb =
        device.events_transport == odyssey_hub_common::device::EventsTransport::Wired;
    // Observe both the prop and pending request without spawning signal writes
    // from render. Hooks stay mounted while CONTROL comes and goes.
    use_effect(use_reactive!(|(device_is_usb)| {
        let requested = *user_pending_usb.read();
        if requested == Some(device_is_usb) {
            user_pending_usb.set(None);
            status.set(OpStatus::Idle);
        }
    }));
    let pending_usb = user_pending_usb.read().unwrap_or(device_is_usb);

    let set_mode = Callback::new(move |usb_mode: bool| {
        spawn(switch_transport_mode(
            hub,
            device.clone(),
            usb_mode,
            status,
            user_pending_usb,
            switching,
        ));
    });

    if !should_render {
        return rsx! {};
    }

    rsx! {
        details {
            class: "mt-2",
            summary {
                class: DISCLOSURE_SUMMARY_CLASS,
                "Connection"
            }
            div {
                class: "mt-2 space-y-2 p-3 bg-gray-100 dark:bg-gray-700 rounded-lg",
                div {
                    class: FIELD_GRID_CLASS,
                    label { class: LABEL_CLASS, "Transport mode" }
                    div {
                        class: "flex gap-3",
                        button {
                            class: mode_button_class(!pending_usb),
                            disabled: switching() || !pending_usb,
                            onclick: move |_| set_mode.call(false),
                            "Bluetooth"
                        }
                        button {
                            class: mode_button_class(pending_usb),
                            disabled: switching() || pending_usb,
                            onclick: move |_| set_mode.call(true),
                            "USB"
                        }
                    }
                }
                p {
                    class: HELP_TEXT_CLASS,
                    if pending_usb {
                        "USB carries sensor data and tuning over the wired connection."
                    } else {
                        "Bluetooth carries sensor data and tuning through a paired Odyssey dongle."
                    }
                }
                StatusLine { status: status() }
            }
        }
    }
}
