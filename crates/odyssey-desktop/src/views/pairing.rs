//! The Pairing page: every dongle and device that can be paired.

mod device_row;
mod dongle_row;
mod pair_buttons;
mod pairing_state;

use device_row::DeviceRow;
use dioxus::prelude::*;
use dongle_row::DongleRow;
use odyssey_hub_common::device::Device;

use crate::hub::HubContext;

#[component]
pub fn Pairing() -> Element {
    let hub = use_context::<Signal<HubContext>>();
    let devices = (hub().devices)();
    let dongles = (hub().dongles)();

    let ats_devices: Vec<Device> = devices.iter().map(|(_slot, d)| d.clone()).collect();

    rsx! {
        div {
            class: "min-h-screen flex flex-col items-center justify-center bg-gray-50 dark:bg-gray-800",
            div {
                class: "w-full max-w-md flex flex-col items-stretch gap-4",
                h1 {
                    class: "text-center text-2xl font-bold text-gray-900 dark:text-white",
                    "Pairing"
                }

                if dongles.is_empty() && ats_devices.is_empty() {
                    p {
                        class: "text-center text-gray-500 dark:text-gray-400",
                        "No devices connected"
                    }
                }

                if !dongles.is_empty() {
                    div {
                        class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow",
                        h2 {
                            class: "text-lg font-semibold text-gray-900 dark:text-white mb-3",
                            "Dongles"
                        }
                        div {
                            class: "space-y-2",
                            for dongle in dongles {
                                DongleRow {
                                    key: "{dongle.id}",
                                    hub,
                                    dongle,
                                }
                            }
                        }
                    }
                }

                if !ats_devices.is_empty() {
                    div {
                        class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow",
                        h2 {
                            class: "text-lg font-semibold text-gray-900 dark:text-white mb-3",
                            "Devices"
                        }
                        div {
                            class: "space-y-2",
                            for ats_dev in ats_devices {
                                DeviceRow {
                                    key: "{ats_dev.uuid:02x?}",
                                    hub,
                                    device: ats_dev,
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
