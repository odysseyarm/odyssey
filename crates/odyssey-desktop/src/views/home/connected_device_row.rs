//! A single row in the connected devices list.
//!
//! A component of its own because Dioxus hooks must run the same number of
//! times, in the same order, on every render of a component instance. A
//! `use_signal` per iteration of a loop over connected devices breaks that as
//! soon as the device count changes, which is when a device and its settings
//! panel first appear.

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};
use odyssey_hub_server::firmware::FirmwareManifest;

use crate::components::{
    DeviceFirmwareUpdate, FirmwareUpdateManager, SensorSettings, TransportModePanel,
};
use crate::hub::HubContext;

/// The firmware version shown for a device until it reports its own.
const UNKNOWN_FIRMWARE_VERSION: [u16; 3] = [0, 0, 0];

fn transport_name(transport: Transport) -> &'static str {
    match transport {
        Transport::Usb => "USB",
        Transport::UsbMux => "BLE",
        Transport::UdpMux => "UDP",
    }
}

/// What to add after the transport name when the device is reached some other
/// way than the name says.
fn transport_suffix(device: &Device) -> &'static str {
    let has_control = device.capabilities.contains(DeviceCapabilities::CONTROL);
    match (device.transport, device.events_transport) {
        (Transport::Usb, EventsTransport::Bluetooth) => " (BLE events)",
        (Transport::UsbMux, _) if has_control => " (USB control)",
        _ => "",
    }
}

fn status_dot_class(is_updating: bool, is_dfu: bool) -> &'static str {
    if is_updating {
        "w-2 h-2 bg-yellow-500 rounded-full animate-pulse"
    } else if is_dfu {
        "w-2 h-2 bg-orange-500 rounded-full animate-pulse"
    } else {
        "w-2 h-2 bg-green-500 rounded-full"
    }
}

#[component]
pub(super) fn ConnectedDeviceRow(
    hub: Signal<HubContext>,
    device: Device,
    is_updating: bool,
    battery: Option<(u8, bool)>,
    manifest: Signal<Option<FirmwareManifest>>,
    manager: FirmwareUpdateManager,
) -> Element {
    let is_dfu = device.capabilities.contains(DeviceCapabilities::DFU)
        && !device.capabilities.contains(DeviceCapabilities::CONTROL);
    let firmware_version = device.firmware_version;

    rsx! {
        div {
            class: "flex flex-col p-3 bg-gray-50 dark:bg-gray-600 rounded gap-2",
            div {
                class: "flex items-center justify-between",
                div {
                    class: "flex items-center gap-3",
                    div { class: status_dot_class(is_updating, is_dfu) }
                    DeviceName { hub, device: device.clone() }
                }
                span {
                    class: "text-xs text-gray-500 dark:text-gray-400",
                    {transport_name(device.transport)}
                    {transport_suffix(&device)}
                }
            }
            div {
                class: "flex items-center gap-2 text-xs",
                span {
                    class: "text-gray-500 dark:text-gray-400",
                    "Firmware:"
                }
                if firmware_version != UNKNOWN_FIRMWARE_VERSION {
                    span {
                        class: "text-gray-700 dark:text-gray-200 font-mono",
                        "{firmware_version[0]}.{firmware_version[1]}.{firmware_version[2]}"
                    }
                } else {
                    span {
                        class: "text-gray-400 dark:text-gray-500 italic",
                        "Not available"
                    }
                }
            }
            if let Some((percent, charging)) = battery {
                div {
                    class: "flex items-center gap-2 text-xs",
                    span {
                        class: "text-gray-500 dark:text-gray-400",
                        "Battery:"
                    }
                    span {
                        class: "text-gray-700 dark:text-gray-200 font-mono",
                        "{percent}%"
                    }
                    span {
                        class: if charging { "text-green-500" } else { "text-gray-400 dark:text-gray-500" },
                        if charging { "Charging" } else { "Discharging" }
                    }
                }
            }
            DeviceFirmwareUpdate {
                device: device.clone(),
                manifest,
                manager,
                devices: hub().devices,
            }
            // Transport mode first: it decides whether sensor tuning is
            // reachable at all, so it belongs above the settings it gates.
            TransportModePanel {
                hub,
                device: device.clone(),
            }
            SensorSettings {
                hub,
                device: device.clone(),
            }
        }
    }
}

/// The device's label, which turns into a text field to rename it.
/// One rename is sent at a time, with an owned client handle rather than a
/// signal write guard held across the RPC.
#[component]
fn DeviceName(hub: Signal<HubContext>, device: Device) -> Element {
    let label = crate::views::device_label(&device);
    let current_name = device.name().to_string();
    let mut editing = use_signal(|| false);
    let mut edit_value = use_signal(|| current_name.clone());
    let mut renamed = use_signal(|| false);
    let mut rename_error = use_signal(|| None::<String>);
    let mut rename_pending = use_signal(|| false);
    if editing() {
        return rsx! {
            input {
                class: "text-sm font-mono text-gray-700 dark:text-gray-200 bg-white dark:bg-gray-700 border border-blue-400 rounded px-1 w-48",
                value: "{edit_value}",
                autofocus: true,
                oninput: move |e| edit_value.set(e.value()),
                onkeydown: {
                    let device = device.clone();
                    move |e: KeyboardEvent| {
                        if e.key() == Key::Enter {
                            if *rename_pending.peek() {
                                return;
                            }
                            rename_pending.set(true);
                            let name = edit_value();
                            let device = device.clone();
                            let mut client = hub.peek().client.peek().clone();
                            editing.set(false);
                            spawn(async move {
                                match client.set_device_name(device, name).await {
                                    Ok(()) => renamed.set(true),
                                    Err(e) => {
                                        tracing::error!("Failed to rename device: {e}");
                                        rename_error.set(Some(e.to_string()));
                                    }
                                }
                                rename_pending.set(false);
                            });
                        } else if e.key() == Key::Escape {
                            editing.set(false);
                        }
                    }
                },
            }
            button {
                class: "text-xs text-gray-400 hover:text-gray-600 dark:hover:text-gray-200",
                onclick: move |_| editing.set(false),
                "✕"
            }
        };
    }
    rsx! {
        span {
            class: "text-sm font-mono text-gray-700 dark:text-gray-200",
            {label}
        }
        button {
            class: "text-xs text-gray-400 hover:text-blue-500 dark:hover:text-blue-400",
            disabled: rename_pending(),
            title: "Rename",
            onclick: move |_| {
                edit_value.set(current_name.clone());
                renamed.set(false);
                rename_error.set(None);
                editing.set(true);
            },
            "✎"
        }
        if renamed() {
            span {
                class: "text-xs text-yellow-500 dark:text-yellow-400 italic",
                "USB name reflects last boot"
            }
        }
        if let Some(error) = rename_error() {
            span { role: "alert", class: "text-xs text-red-500", {error} }
        }
    }
}
