use dioxus::prelude::*;
use crate::styles::*;
use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use std::time::Duration;
use tokio::time;

#[component]
pub fn Accessories() -> Element {
    let devices = use_signal(|| Vec::<String>::new());
    let scanning = use_signal(|| false);

    rsx! {
        div {
            class: PAGE_CONTAINER,

            div {
                class: HEADER_CONTAINER,

                h1 {
                    class: HEADING,
                    "Accessories"
                }

                button {
                    class: BUTTON_PRIMARY,
                    onclick: {
                        to_owned![devices, scanning];
                        move |_| {
                            scanning.set(true);

                            spawn(async move {
                                let found = scan_for_devices().await;
                                devices.set(found);
                                scanning.set(false);
                            });
                        }
                    },
                    if scanning() {
                        "Scanning..."
                    } else {
                        "Add New Device"
                    }
                }
            }

            div {
                class: CONTENT_CONTAINER,

                if devices().is_empty() {
                    div {
                        class: EMPTY_STATE_TEXT,
                        "No accessories found. Click \"Add New Device\" to scan."
                    }
                } else {
                    ul {
                        class: DEVICE_LIST_WRAPPER,
                        for device in devices().iter() {
                            li {
                                class: DEVICE_LIST_ITEM,
                                "{device}"
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn scan_for_devices() -> Vec<String> {
    let manager = Manager::new().await.unwrap();
    let adapters = manager.adapters().await.unwrap();
    let central = adapters.into_iter().nth(0).unwrap();

    central.start_scan(ScanFilter::default()).await.unwrap();
    time::sleep(Duration::from_secs(3)).await;
    central.stop_scan().await.unwrap();

    let mut found = Vec::new();
    for p in central.peripherals().await.unwrap() {
        if let Some(props) = p.properties().await.unwrap() {
            if let Some(name) = props.local_name {
                found.push(name);
            }
        }
    }

    found
}
