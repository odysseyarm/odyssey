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
    DeviceFirmwareUpdate, FirmwareUpdateManager, ImageViewer, SensorSettings, TransportModePanel,
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
        "size-2 bg-accent rounded-full animate-pulse"
    } else if is_dfu {
        "size-2 bg-warning rounded-full animate-pulse"
    } else {
        "size-2 bg-success rounded-full"
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
            class: "flex flex-col p-3 bg-surface-alt rounded-lg gap-2",
            div {
                class: "flex items-center justify-between",
                div {
                    class: "flex items-center gap-3",
                    div { class: status_dot_class(is_updating, is_dfu) }
                    DeviceName { hub, device: device.clone() }
                }
                span {
                    class: "text-xs text-fg-muted",
                    {transport_name(device.transport)}
                    {transport_suffix(&device)}
                }
            }
            div {
                class: "flex items-center gap-2 text-xs",
                span {
                    class: "text-fg-muted",
                    "Firmware:"
                }
                if firmware_version != UNKNOWN_FIRMWARE_VERSION {
                    span {
                        class: "text-fg font-mono",
                        "{firmware_version[0]}.{firmware_version[1]}.{firmware_version[2]}"
                    }
                } else {
                    span {
                        class: "text-fg-muted italic",
                        "Not available"
                    }
                }
            }
            if let Some((percent, charging)) = battery {
                div {
                    class: "flex items-center gap-2 text-xs",
                    span {
                        class: "text-fg-muted",
                        "Battery:"
                    }
                    span {
                        class: "text-fg font-mono",
                        "{percent}%"
                    }
                    span {
                        class: if charging { "text-success-ink" } else { "text-fg-muted" },
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
            ImageViewer {
                hub,
                device: device.clone(),
                is_updating,
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
                class: "field px-1 py-0.5 font-mono w-48",
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
                            let client = hub.peek().client();
                            editing.set(false);
                            spawn(async move {
                                let Some(client) = client else {
                                    tracing::warn!("Failed to rename device: not connected to the hub");
                                    rename_error.set(Some("not connected to the hub".to_owned()));
                                    rename_pending.set(false);
                                    return;
                                };
                                match client.device(device).set_device_name(name).await {
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
                class: "text-xs text-fg-muted hover:text-fg-strong",
                onclick: move |_| editing.set(false),
                "✕"
            }
        };
    }
    rsx! {
        span {
            class: "text-sm font-mono text-fg",
            {label}
        }
        button {
            class: "text-xs text-fg-muted hover:text-accent-ink",
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
                class: "text-xs text-warning-ink italic",
                "USB name reflects last boot"
            }
        }
        if let Some(error) = rename_error() {
            span { role: "alert", class: "text-xs text-danger-ink", {error} }
        }
    }
}
