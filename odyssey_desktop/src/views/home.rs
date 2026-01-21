use dioxus::{
    desktop::{tao::window::Fullscreen, WindowBuilder},
    prelude::*,
};

use crate::components::crosshair_manager::TrackingSender;
use crate::hub;

#[component]
pub fn Home() -> Element {
    let hub = use_context::<Signal<hub::HubContext>>();

    let window = dioxus::desktop::use_window();
    let options = use_memo(move || window.available_monitors().collect::<Vec<_>>());

    let mut selected_index = use_signal(|| 0_usize);
    let selected_value = use_memo(move || options().get(selected_index()).cloned());

    let devices = (hub().devices)();

    rsx! {
        div {
            class: "min-h-screen flex flex-col items-center justify-center bg-gray-50 dark:bg-gray-800",

            div {
                class: "w-full max-w-md flex flex-col items-stretch gap-4",

                h1 {
                    class: "text-center text-2xl font-bold text-gray-900 dark:text-white",
                    "Welcome!"
                }

                // Connected devices section
                if !devices.is_empty() {
                    div {
                        class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow",
                        h2 {
                            class: "text-lg font-semibold text-gray-900 dark:text-white mb-3",
                            "Connected Devices"
                        }
                        div {
                            class: "space-y-2",
                            for (_slot, device) in devices.iter() {
                                div {
                                    class: "flex flex-col p-3 bg-gray-50 dark:bg-gray-600 rounded gap-2",
                                    div {
                                        class: "flex items-center justify-between",
                                        div {
                                            class: "flex items-center gap-3",
                                            div {
                                                class: "w-2 h-2 bg-green-500 rounded-full"
                                            }
                                            span {
                                                class: "text-sm font-mono text-gray-700 dark:text-gray-200",
                                                "{device.uuid[0]:02x}:{device.uuid[1]:02x}:{device.uuid[2]:02x}:{device.uuid[3]:02x}:{device.uuid[4]:02x}:{device.uuid[5]:02x}"
                                            }
                                        }
                                        span {
                                            class: "text-xs text-gray-500 dark:text-gray-400",
                                            // Show primary transport
                                            match device.transport {
                                                odyssey_hub_common::device::Transport::Usb => "USB",
                                                odyssey_hub_common::device::Transport::UsbMux => "BLE",
                                                odyssey_hub_common::device::Transport::UdpMux => "UDP",
                                            }
                                            // Show additional transport info
                                            {
                                                use odyssey_hub_common::device::DeviceCapabilities;
                                                match (device.transport, device.events_transport, device.events_connected, device.capabilities.contains(DeviceCapabilities::CONTROL)) {
                                                    // USB transport with BLE events
                                                    (odyssey_hub_common::device::Transport::Usb, odyssey_hub_common::device::EventsTransport::Bluetooth, true, _) => " (BLE events ✓)",
                                                    (odyssey_hub_common::device::Transport::Usb, odyssey_hub_common::device::EventsTransport::Bluetooth, false, _) => " (BLE events ✗)",
                                                    // BLE mux transport with USB control
                                                    (odyssey_hub_common::device::Transport::UsbMux, _, _, true) => " (USB control ✓)",
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
                                        if let Some(ref fw) = device.firmware_version {
                                            span {
                                                class: "text-gray-700 dark:text-gray-200 font-mono",
                                                "{fw[0]}.{fw[1]}.{fw[2]}"
                                            }
                                        } else {
                                            span {
                                                class: "text-gray-400 dark:text-gray-500 italic",
                                                "Not available"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                form {
                    class: "flex flex-col text-gray-500 dark:text-gray-400 gap-4",

                    label {
                        class: "text-base font-medium",
                        "Choose a display:"
                    }

                    div {
                        class: "flex flex-row items-center gap-2",

                        select {
                            class: "bg-gray-50 border border-gray-300 text-gray-900 text-base rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-gray-700 dark:border-gray-600 dark:placeholder-gray-400 dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500",
                            onchange: move |evt| {
                                if let Ok(index) = evt.value().parse::<usize>() {
                                    if index < options().len() {
                                        selected_index.set(index);
                                    }
                                }
                            },
                            option { value: "", disabled: true, selected: selected_value().is_none(), "Select an option" },
                            for (i, display) in options().iter().enumerate() {
                                option {
                                    value: "{i}",
                                    "{display.name().unwrap_or(\"Unknown Display\".to_string())}",
                                }
                            }
                        }

                        button {
                            class: "w-12 shrink-0 text-white bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 font-medium rounded-lg text-base p-2.5 dark:bg-blue-600 dark:hover:bg-blue-700 focus:outline-none dark:focus:ring-blue-800",
                            r#type: "button",
                            onclick: move |_| {
                                // why is async necessary?
                                async move {
                                    if let Some(sel) = selected_value() {
                                        // Clone the tracking sender to pass directly to the new window
                                        // This avoids Signal issues across VirtualDoms
                                        let tracking_sender = TrackingSender(hub.peek().tracking_events.clone());
                                        let dom = VirtualDom::new_with_props(crate::views::Zero, crate::views::zero::ZeroProps { hub, tracking_sender });
                                        // HACK: with_position is necessary for reading the correct scale factor off the window
                                        let config = dioxus::desktop::Config::default().with_menu(None).with_close_behaviour(dioxus::desktop::WindowCloseBehaviour::WindowCloses).with_window(WindowBuilder::new().with_position(sel.position()).with_fullscreen(Some(Fullscreen::Borderless(Some(sel)))));
                                        dioxus::desktop::window().new_window(dom, config);
                                    }
                                }
                            },
                            "Go"
                        }
                    }
                }
            }
        }
    }
}
