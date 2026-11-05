//! The Pairing page: every dongle and device that can be paired.

mod device_row;
mod dongle_row;
mod pair_buttons;
mod pairing_state;

use device_row::DeviceRow;
use dioxus::prelude::*;
use dongle_row::DongleRow;
use odyssey_hub_common::device::Device;

use crate::components::Page;
use crate::hub::HubContext;

#[component]
pub fn Pairing() -> Element {
    let hub = use_context::<Signal<HubContext>>();
    let devices = (hub().devices)();
    let dongles = (hub().dongles)();

    let ats_devices: Vec<Device> = devices.iter().map(|(_slot, d)| d.clone()).collect();

    rsx! {
        Page {
            title: "Pairing",
            description: "Pair a device with a dongle to use it without a cable. A dongle remembers the devices it has bonded with.",

            if dongles.is_empty() && ats_devices.is_empty() {
                div {
                    class: "card",
                    h2 { class: "card-title mb-1", "Nothing to pair yet" }
                    p {
                        class: "text-sm text-fg-muted",
                        "Plug a dongle or a device into this computer. They appear here as soon as the hub finds them."
                    }
                }
            }

            if !dongles.is_empty() {
                div {
                    class: "card",
                    h2 { class: "card-title mb-3", "Dongles" }
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
                    class: "card",
                    h2 { class: "card-title mb-3", "Devices" }
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
