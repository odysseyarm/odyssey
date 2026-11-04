use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::{Device, DeviceCapabilities};

use super::number_field::LABEL_CLASS;
use super::op_status::{OpStatus, StatusLine, rpc_failure};
use super::product_ids::{PID_ATS_LITE, PID_ATS_PRO};
use crate::hub::HubContext;

/// Shows a transport mode selector (BLE / USB) for ATS Lite / Lite1 devices.
/// Requires only CONTROL capability, so it is visible even when the device is
/// in BLE mode and the EVENTS bulk endpoint is not available.
#[component]
pub(crate) fn TransportModePanel(hub: Signal<HubContext>, device: Device) -> Element {
    let pid = device.product_id;
    // Only ATS Lite (0x5210) and Lite1 (0x5211) support transport mode switching.
    if pid != PID_ATS_LITE && pid != PID_ATS_PRO {
        return rsx! {};
    }
    if !device.capabilities.contains(DeviceCapabilities::CONTROL) {
        return rsx! {};
    }

    let mut status = use_signal(OpStatus::default);
    // user_pending_usb tracks the mode the user explicitly requested (None = no override).
    // It is cleared when the device prop reflects the new mode (i.e. after reboot).
    let mut user_pending_usb = use_signal(|| Option::<bool>::None);

    // Derive the displayed mode: prefer the user override while waiting for reboot,
    // otherwise use the actual device state from the prop (updates on every render).
    let device_is_usb =
        device.events_transport == odyssey_hub_common::device::EventsTransport::Wired;
    // Once the device reflects the user's requested mode, drop the override and clear the status.
    // We schedule this via spawn so we don't write to signals during the render phase.
    if let Some(u) = *user_pending_usb.read()
        && u == device_is_usb
    {
        spawn(async move {
            user_pending_usb.set(None);
            status.set(OpStatus::Idle);
        });
    }
    let pending_usb = user_pending_usb.read().unwrap_or(device_is_usb);

    let device_for_set = device.clone();
    let set_mode = std::sync::Arc::new(move |usb_mode: bool| {
        let device = device_for_set.clone();
        async move {
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
                    status.set(rpc_failure("switch the transport mode", &e));
                }
            }
        }
    });

    rsx! {
        details {
            class: "mt-2",
            summary {
                class: "text-sm font-medium text-gray-700 dark:text-gray-300 cursor-pointer \
                        hover:text-blue-600 dark:hover:text-blue-400 select-none",
                "Connection"
            }
            div {
                class: "mt-2 space-y-2 p-3 bg-gray-100 dark:bg-gray-700 rounded-lg",
                div {
                    class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center",
                    label { class: LABEL_CLASS, "Transport mode" }
                    div {
                        class: "flex gap-3",
                        button {
                            class: if !pending_usb {
                                "btn-primary text-xs px-2 py-1"
                            } else {
                                "btn-secondary text-xs px-2 py-1"
                            },
                            onclick: { let set_mode = set_mode.clone(); move |_| set_mode(false) },
                            "Bluetooth"
                        }
                        button {
                            class: if pending_usb {
                                "btn-primary text-xs px-2 py-1"
                            } else {
                                "btn-secondary text-xs px-2 py-1"
                            },
                            onclick: move |_| set_mode(true),
                            "USB"
                        }
                    }
                }
                p {
                    class: "text-xs text-gray-500 dark:text-gray-400",
                    if pending_usb {
                        "USB carries sensor data and tuning. Bluetooth leaves the device wireless but limits it to basic settings."
                    } else {
                        "Bluetooth keeps the device wireless. Switch to USB to read or change sensor tuning."
                    }
                }
                StatusLine { status: status() }
            }
        }
    }
}
