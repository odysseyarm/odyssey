use dioxus::{logger::tracing, prelude::*};
use odyssey_hub_server::firmware::FirmwareChannel;

use crate::components::firmware_update::FIRMWARE_CHANNEL;

#[component]
pub fn Settings() -> Element {
    use_future(|| async {
        if FIRMWARE_CHANNEL.peek().is_none() {
            *FIRMWARE_CHANNEL.write() = Some(FirmwareChannel::load().await);
        }
    });
    let current = FIRMWARE_CHANNEL().unwrap_or_default();
    let choose = move |channel: FirmwareChannel| {
        *FIRMWARE_CHANNEL.write() = Some(channel);
        spawn(async move {
            if let Err(e) = channel.save().await {
                tracing::warn!("Could not save the firmware channel: {e}");
            }
        });
    };
    rsx! {
        div { class: "min-h-screen bg-gray-50 dark:bg-gray-800 p-6",
            div { class: "w-full max-w-xl mx-auto flex flex-col gap-4",
                h1 { class: "text-2xl font-bold text-gray-900 dark:text-white", "Settings" }

                div { class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow space-y-4",
                    h2 { class: "text-lg font-semibold text-gray-900 dark:text-white", "About" }

                    p { class: "text-sm text-gray-600 dark:text-gray-300",
                        {format!("Odyssey Desktop v{}", env!("CARGO_PKG_VERSION"))}
                    }
                }

                div { class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow space-y-4",
                    h2 { class: "text-lg font-semibold text-gray-900 dark:text-white", "Firmware updates" }

                    for (channel, label, help) in [
                        (FirmwareChannel::Stable, "Stable", "Firmware that has been tested and promoted. Recommended."),
                        (FirmwareChannel::Alpha, "Alpha", "Every firmware release as soon as it is published. May contain unfinished changes."),
                    ] {
                        label { class: "flex items-start gap-3 cursor-pointer",
                            input {
                                r#type: "radio",
                                name: "firmware-channel",
                                checked: current == channel,
                                onchange: move |_| choose(channel),
                            }
                            span {
                                span { class: "block text-sm font-medium text-gray-900 dark:text-white", "{label}" }
                                span { class: "block text-sm text-gray-600 dark:text-gray-300", "{help}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
