use dioxus::desktop::WindowBuilder;
use dioxus::desktop::tao::window::Fullscreen;
use dioxus::logger::tracing;
use dioxus::prelude::*;
use dioxus_router::Link;
use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};
use odyssey_hub_server::firmware::{self, FirmwareManifest};

use crate::components::crosshair_manager::TrackingSender;
use crate::components::firmware_update::{FIRMWARE_CHANNEL, ensure_firmware_channel_loaded};
use crate::components::{
    DeviceFirmwareUpdate, FirmwareUpdateManager, ScreenPicker, SensorSettings, TransportModePanel,
    UpdatingDeviceRow, use_monitor_selection,
};
use crate::{Route, hub};

#[component]
pub fn Home() -> Element {
    let hub = use_context::<Signal<hub::HubContext>>();

    let monitor_selection = use_monitor_selection();
    let selected_monitor = move || monitor_selection.selected_handle();

    let devices = (hub().devices)();
    let battery_states = (hub().battery_states)();

    // Firmware manifest - fetched once and shared across all device firmware update components
    let manifest: Signal<Option<FirmwareManifest>> = use_signal(|| None);
    // Why there is no manifest, for the offline notice. The retrying itself is only logged.
    let manifest_error: Signal<Option<String>> = use_signal(|| None);

    // Firmware update manager - shared state between tokio tasks and UI
    let manager = use_hook(FirmwareUpdateManager::new);

    // Signal to trigger re-renders when updating devices change
    let mut updating_uuids = use_signal(Vec::<[u8; 6]>::new);

    // Poll for updating devices
    use_future({
        let manager = manager.clone();
        move || {
            let manager = manager.clone();
            async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    let uuids = manager.get_all_updating();
                    updating_uuids.set(uuids);
                }
            }
        }
    });

    // Without the manifest no update is ever offered, and one failed fetch used
    // to disable updates for the rest of the session. Keep trying, backing off
    // so an offline machine is not hammering the network.
    use_future(ensure_firmware_channel_loaded);
    use_resource({
        let mut manifest = manifest.clone();
        let mut manifest_error = manifest_error;
        move || async move {
            // Reading the signal here re-runs this when Settings changes it.
            let Some(channel) = FIRMWARE_CHANNEL() else {
                return;
            };
            manifest.set(None);
            let mut delay = std::time::Duration::from_secs(5);
            loop {
                match firmware::fetch_manifest(channel).await {
                    Ok(m) => {
                        tracing::info!(
                            "Fetched {} firmware manifest version {}",
                            channel.name(),
                            m.version
                        );
                        manifest_error.set(None);
                        manifest.set(Some(m));
                        return;
                    }
                    Err(e) => {
                        manifest_error.set(Some(e.to_string()));
                        tracing::warn!("Failed to fetch firmware manifest, retrying: {}", e);
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(std::time::Duration::from_secs(300));
                    }
                }
            }
        }
    });

    let updating = updating_uuids();

    rsx! {
        div {
            class: "min-h-screen flex flex-col items-center justify-center bg-gray-50 dark:bg-gray-800",

            div {
                class: "w-full max-w-md flex flex-col items-stretch gap-4",

                h1 {
                    class: "text-center text-2xl font-bold text-gray-900 dark:text-white",
                    "Welcome!"
                }

                OfflineNotice { error: manifest_error() }

                // Connected devices section
                {
                    let has_devices = !devices.is_empty() || !updating.is_empty();

                    if has_devices {
                        rsx! {
                            div {
                                class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow",
                                h2 {
                                    class: "text-lg font-semibold text-gray-900 dark:text-white mb-3",
                                    "Connected Devices"
                                }
                                div {
                                    class: "space-y-2",
                                    for (_slot, device) in devices.iter() {
                                        {
                                            let is_updating = updating.contains(&device.uuid);
                                            let battery = battery_states.get(&device.uuid).copied();
                                            rsx! {
                                                ConnectedDeviceRow {
                                                    key: "{device.uuid:02x?}",
                                                    hub: hub,
                                                    device: device.clone(),
                                                    is_updating: is_updating,
                                                    battery: battery,
                                                    manifest: manifest,
                                                    manager: manager.clone(),
                                                }
                                            }
                                        }
                                    }
                                    // Show devices being updated but not in device list
                                    {
                                        let updating_not_in_list: Vec<[u8; 6]> = updating
                                            .iter()
                                            .filter(|uuid| !devices.iter().any(|(_, d)| &d.uuid == *uuid))
                                            .copied()
                                            .collect();
                                        rsx! {
                                            for uuid in updating_not_in_list {
                                                UpdatingDeviceRow {
                                                    key: "{uuid:02x?}",
                                                    uuid: uuid,
                                                    manager: manager.clone(),
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        rsx! {}
                    }
                }

                div {
                    class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow flex flex-col gap-3",

                    ScreenPicker { selection: monitor_selection }

                    div {
                        class: "flex flex-row justify-end gap-3",
                        Link {
                            to: Route::Calibrate {},
                            class: "shrink-0 text-gray-900 bg-white border border-gray-300 hover:bg-gray-100 focus:ring-4 focus:ring-gray-200 font-medium rounded-lg text-base px-5 py-2.5 focus:outline-none dark:bg-gray-800 dark:text-white dark:border-gray-600 dark:hover:bg-gray-700 dark:focus:ring-gray-700",
                            "Calibrate screen"
                        }
                        button {
                            class: "shrink-0 text-white bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 font-medium rounded-lg text-base px-5 py-2.5 dark:bg-blue-600 dark:hover:bg-blue-700 focus:outline-none dark:focus:ring-blue-800 disabled:opacity-50 disabled:pointer-events-none",
                            r#type: "button",
                            disabled: monitor_selection.selected().is_none(),
                            onclick: move |_| {
                                async move {
                                    if let Some(sel) = selected_monitor() {
                                        let tracking_sender = TrackingSender(hub.peek().tracking_events.clone());
                                        let dom = VirtualDom::new_with_props(crate::views::Zero, crate::views::zero::ZeroProps { hub, tracking_sender });
                                        let config = dioxus::desktop::Config::default().with_menu(None).with_close_behaviour(dioxus::desktop::WindowCloseBehaviour::WindowCloses).with_window(WindowBuilder::new().with_position(sel.position()).with_fullscreen(Some(Fullscreen::Borderless(Some(sel)))));
                                        dioxus::desktop::window().new_window(dom, config);
                                    }
                                }
                            },
                            "Zero"
                        }
                    }
                }
            }
        }
    }
}

/// A single row in the connected devices list.
///
/// Split out of `Home`'s render body (rather than looping inline with `use_signal` calls per
/// iteration) because Dioxus hooks must be called the same number of times, in the same order,
/// on every render of a component instance. Calling `use_signal` inside a `for` loop whose
/// length tracks the number of connected devices breaks that invariant as soon as the device
/// count changes — which is exactly when a device (and its settings panel) would first appear.
#[derive(Props, Clone, PartialEq)]
struct ConnectedDeviceRowProps {
    hub: Signal<hub::HubContext>,
    device: Device,
    is_updating: bool,
    battery: Option<(u8, bool)>,
    manifest: Signal<Option<FirmwareManifest>>,
    manager: FirmwareUpdateManager,
}

#[component]
fn ConnectedDeviceRow(props: ConnectedDeviceRowProps) -> Element {
    let hub = props.hub;
    let device = props.device.clone();
    let is_updating = props.is_updating;
    let label = crate::views::device_label(&device);
    let is_dfu = device.capabilities.contains(DeviceCapabilities::DFU)
        && !device.capabilities.contains(DeviceCapabilities::CONTROL);
    let transport = device.transport;
    let events_transport = device.events_transport;
    let events_connected = device.events_connected;
    let firmware_version = device.firmware_version;
    let battery = props.battery;
    let manifest = props.manifest;
    let manager = props.manager.clone();

    let current_name = device.name().to_string();
    let mut editing = use_signal(|| false);
    let mut edit_value = use_signal(|| current_name.clone());
    let mut renamed = use_signal(|| false);

    rsx! {
        div {
            class: "flex flex-col p-3 bg-gray-50 dark:bg-gray-600 rounded gap-2",
            div {
                class: "flex items-center justify-between",
                div {
                    class: "flex items-center gap-3",
                    div {
                        class: if is_updating { "w-2 h-2 bg-yellow-500 rounded-full animate-pulse" } else if is_dfu { "w-2 h-2 bg-orange-500 rounded-full animate-pulse" } else { "w-2 h-2 bg-green-500 rounded-full" }
                    }
                    if editing() {
                        input {
                            class: "text-sm font-mono text-gray-700 dark:text-gray-200 bg-white dark:bg-gray-700 border border-blue-400 rounded px-1 w-48",
                            value: "{edit_value}",
                            autofocus: true,
                            oninput: move |e| edit_value.set(e.value()),
                            onkeydown: {
                                let device = device.clone();
                                move |e: KeyboardEvent| {
                                    if e.key() == Key::Enter {
                                        let name = edit_value();
                                        let device = device.clone();
                                        let mut client = hub().client;
                                        editing.set(false);
                                        renamed.set(true);
                                        spawn(async move {
                                            let _ = client.write().set_device_name(device, name).await;
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
                    } else {
                        span {
                            class: "text-sm font-mono text-gray-700 dark:text-gray-200",
                            "{label}"
                        }
                        button {
                            class: "text-xs text-gray-400 hover:text-blue-500 dark:hover:text-blue-400",
                            title: "Rename",
                            onclick: move |_| {
                                edit_value.set(current_name.clone());
                                renamed.set(false);
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
                    }
                }
                span {
                    class: "text-xs text-gray-500 dark:text-gray-400",
                    match transport {
                        Transport::Usb => "USB",
                        Transport::UsbMux => "BLE",
                        Transport::UdpMux => "UDP",
                    }
                    {
                        match (transport, events_transport, events_connected, device.capabilities.contains(DeviceCapabilities::CONTROL)) {
                            (Transport::Usb, EventsTransport::Bluetooth, true, _) => " (BLE events)",
                            (Transport::Usb, EventsTransport::Bluetooth, false, _) => " (BLE events)",
                            (Transport::UsbMux, _, _, true) => " (USB control)",
                            _ => "",
                        }
                    }
                }
            }
            div {
                class: "flex items-center gap-2 text-xs",
                span {
                    class: "text-gray-500 dark:text-gray-400",
                    "Firmware:"
                }
                if firmware_version != [0, 0, 0] {
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
                manifest: manifest,
                manager: manager.clone(),
                devices: hub().devices,
            }
            // Transport mode first: it decides whether sensor tuning is
            // reachable at all, so it belongs above the settings it gates.
            TransportModePanel {
                hub: hub,
                device: device.clone(),
            }
            SensorSettings {
                hub: hub,
                device: device.clone(),
            }
        }
    }
}

/// Says so when offline mode is on and where firmware updates come from, and
/// shows what is wrong with the firmware pack or the policy file. Shows nothing
/// when everything is online and fine.
#[component]
fn OfflineNotice(error: Option<String>) -> Element {
    let policy = odyssey_hub_server::policy::current();
    if !policy.offline && policy.issues.is_empty() {
        return rsx! {};
    }
    let dir = odyssey_hub_server::policy::firmware_pack_dir()
        .display()
        .to_string();

    rsx! {
        div {
            role: "note",
            class: "flex flex-col gap-1 rounded-lg border-2 border-amber-300 bg-amber-50 p-3 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950/60 dark:text-amber-100",
            if policy.offline {
                span { class: "font-semibold", "Offline mode is on" }
                span {
                    "Odyssey won't connect to the internet. Firmware updates come from the firmware pack in {dir}."
                }
                if let Some(error) = error {
                    span { role: "alert", class: "font-medium", "{error}" }
                }
            }
            for issue in policy.issues.iter() {
                span { key: "{issue}", role: "alert", class: "font-medium", "{issue}" }
            }
        }
    }
}
