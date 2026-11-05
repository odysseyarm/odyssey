use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_server::firmware::FirmwareChannel;

use crate::components::Page;
use crate::components::firmware_update::{FIRMWARE_CHANNEL, ensure_firmware_channel_loaded};

#[component]
pub fn Settings() -> Element {
    use_future(ensure_firmware_channel_loaded);
    let current = FIRMWARE_CHANNEL().unwrap_or_default();
    // The choice applies at once; a failed save only means it is lost at the
    // next start, which the user has to be told.
    let mut save_error = use_signal(|| None::<String>);
    let choose = move |channel: FirmwareChannel| {
        *FIRMWARE_CHANNEL.write() = Some(channel);
        spawn(async move {
            match channel.save().await {
                Ok(()) => save_error.set(None),
                Err(e) => {
                    tracing::warn!("Could not save the firmware channel: {e}");
                    save_error.set(Some(e.to_string()));
                }
            }
        });
    };
    rsx! {
        Page {
            title: "Settings",

            div { class: "card space-y-4",
                h2 { class: "card-title", "Firmware updates" }

                for (channel, label, help) in [
                    (FirmwareChannel::Stable, "Stable", "Firmware that has been tested and promoted. Recommended."),
                    (FirmwareChannel::Alpha, "Alpha", "Every firmware release as soon as it is published. May contain unfinished changes."),
                ] {
                    label { class: "flex items-start gap-3 cursor-pointer",
                        input {
                            class: "mt-1",
                            r#type: "radio",
                            name: "firmware-channel",
                            checked: current == channel,
                            onchange: move |_| choose(channel),
                        }
                        span {
                            span { class: "block text-sm font-medium text-fg-strong", {label} }
                            span { class: "block text-sm text-fg-muted", {help} }
                        }
                    }
                }

                if let Some(error) = save_error() {
                    p {
                        class: "text-sm text-danger-ink",
                        role: "alert",
                        "Applied for now, but could not be saved, so it resets when the app restarts: {error}"
                    }
                }
            }

            div { class: "card space-y-1",
                h2 { class: "card-title", "About" }
                p { class: "text-sm text-fg-muted",
                    {format!("Odyssey Desktop v{}", env!("CARGO_PKG_VERSION"))}
                }
            }
        }
    }
}
