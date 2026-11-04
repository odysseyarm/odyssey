//! USB image viewer. A mounted LiveSession exclusively owns its stream;
//! Stop, dialog close, row removal, or a lost USB link unmounts it and cancels
//! the RPC. Modern tracking resumes; legacy PAJ7025 requires reboot/reconnect.
//! Camera calibration deliberately remains in odyssey-vision-tool.

mod frame;
mod session;

use std::time::{Duration, Instant};

use dioxus::prelude::*;
use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport};
use odyssey_protocol::Port;

use self::frame::{FrameView, PresentedFrame};
use self::session::LiveSession;
use crate::hub::HubContext;

const BUTTON: &str = "px-3 py-1 text-sm rounded bg-gray-200 hover:bg-gray-300 dark:bg-gray-600 dark:hover:bg-gray-500 disabled:opacity-50";
const HELP: &str = "text-sm text-gray-600 dark:text-gray-300";

fn unavailable(device: &Device, is_updating: bool) -> Option<&'static str> {
    if is_updating {
        Some("Image capture is unavailable during a firmware update.")
    } else if device.capabilities.contains(DeviceCapabilities::DFU) {
        Some("Image capture requires normal application firmware, not bootloader mode.")
    } else if !device.capabilities.contains(DeviceCapabilities::CONTROL)
        || !device.capabilities.contains(DeviceCapabilities::EVENTS)
        || device.events_transport != EventsTransport::Wired
    {
        Some(
            "USB only: connect this device directly by USB and select USB sensor events. BLE and dongle links cannot carry images.",
        )
    } else if !device.events_connected {
        Some("The USB sensor connection is not available. Capture has stopped.")
    } else {
        None
    }
}

#[component]
pub(crate) fn ImageViewer(hub: Signal<HubContext>, device: Device, is_updating: bool) -> Element {
    let mut open = use_signal(|| false);
    let needs_reboot = use_signal(|| false);
    // Keep the action reachable even over BLE so the USB requirement is visible.
    let known_sensor = matches!(device.product_id, 0x520f..=0x5211);
    rsx! {
        if known_sensor {
            div {
                button {
                    class: BUTTON,
                    onclick: move |_| open.set(true),
                    "Image / IR"
                }
                if needs_reboot() {
                    span { class: "text-sm text-amber-700 dark:text-amber-300", "Legacy image mode: reboot/reconnect required" }
                }
            }
            if open() {
                ImageDialog {
                    hub,
                    device,
                    is_updating,
                    needs_reboot,
                    on_close: move |()| open.set(false),
                }
            }
        }
    }
}

#[component]
fn ImageDialog(
    hub: Signal<HubContext>,
    device: Device,
    is_updating: bool,
    on_close: EventHandler<()>,
    mut needs_reboot: Signal<bool>,
) -> Element {
    let mut camera = use_signal(|| Port::Nf);
    let mut fps = use_signal(|| 15u8);
    let mut running = use_signal(|| false);
    let mut status = use_signal(|| "Stopped.".to_owned());
    let mut error = use_signal(|| None::<String>);
    let mut latest = use_signal(|| None::<PresentedFrame>);
    let mut now = use_signal(Instant::now);
    let mut rebooting = use_signal(|| false);
    let reason = unavailable(&device, is_updating);
    let can_stream = reason.is_none();
    let supports_wf = matches!(device.product_id, 0x520f | 0x5210);
    let label = crate::views::device_label(&device);
    let legacy = supports_wf;
    let reboot_device = device.clone();

    use_effect(use_reactive!(|(can_stream)| {
        if !can_stream {
            running.set(false);
            status.set(if legacy { "USB unavailable. Legacy image mode still requires reboot/power-cycle and reconnect." } else { "Stopped: the USB image connection is unavailable." }.to_owned());
        }
    }));
    // Updates receive age even when no new frames arrive or the view is paused.
    use_future(move || async move {
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            now.set(Instant::now());
        }
    });

    rsx! {
        div {
            class: "fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4",
            onclick: move |_| on_close.call(()),
            div {
                role: "dialog",
                aria_modal: "true",
                aria_label: "Live USB infrared image",
                tabindex: 0,
                class: "flex max-h-full w-full max-w-3xl flex-col gap-4 overflow-y-auto rounded-xl bg-white p-6 shadow-xl outline-none dark:bg-gray-800",
                onclick: move |event| event.stop_propagation(),
                onmounted: move |event| async move { let _ = event.set_focus(true).await; },
                onkeydown: move |event| {
                    if event.key() == Key::Escape { on_close.call(()); }
                },
                div {
                    class: "flex items-center justify-between gap-3",
                    h2 { class: "text-xl font-semibold", "Image / IR — {label}" }
                    button { class: BUTTON, onclick: move |_| on_close.call(()), "Close" }
                }
                p {
                    class: HELP,
                    if legacy {
                        "Legacy PAJ7025: native 98×98 NF/WF images. Entering image mode stops tracking and IMU reports. Stop, close, error or disconnect stops pixels ONLY; reboot/power-cycle and reconnect are REQUIRED to restore tracking or switch camera. Saved settings are unchanged; unsaved sensor edits must be reapplied after reboot."
                    } else {
                        "Image-only capture. Tracking pauses until Stop or close; IMU/control remain available."
                    }
                }
                if let Some(reason) = reason {
                    p { role: "alert", class: "text-sm text-red-700 dark:text-red-300", {reason} }
                }
                div {
                    class: "flex items-center gap-3 flex-wrap",
                    label {
                        class: HELP,
                        "Camera "
                        select {
                            class: "rounded bg-gray-100 dark:bg-gray-700 p-1",
                            aria_label: "Image camera",
                            disabled: running(),
                            value: if camera() == Port::Wf { "wf" } else { "nf" },
                            onchange: move |event| {
                                camera.set(if event.value() == "wf" { Port::Wf } else { Port::Nf });
                                latest.set(None);
                            },
                            option { value: "nf", "NF — near field" }
                            if supports_wf { option { value: "wf", "WF — wide field" } }
                        }
                    }
                    label {
                        class: HELP,
                        "Requested FPS "
                        select {
                            class: "rounded bg-gray-100 dark:bg-gray-700 p-1",
                            aria_label: "Requested image frames per second",
                            disabled: running(),
                            value: "{fps}",
                            onchange: move |event| {
                                if let Ok(value) = event.value().parse::<u8>() { fps.set(value); }
                            },
                            for value in 1u8..=15 { option { value: "{value}", selected: value == fps(), "{value}" } }
                        }
                    }
                    if running() {
                        button {
                            class: BUTTON,
                            onclick: move |_| {
                                running.set(false);
                                status.set(if legacy { "Pixels stopped. Legacy tracking is NOT restored: reboot/power-cycle and reconnect." } else { "Stopped. Last received frame is frozen; normal tracking is being restored." }.to_owned());
                            },
                            "Stop"
                        }
                    } else {
                        button {
                            class: BUTTON,
                            disabled: !can_stream || needs_reboot() || rebooting(),
                            onclick: move |_| {
                                latest.set(None);
                                error.set(None);
                                status.set("Starting USB image session…".to_owned());
                                if legacy { needs_reboot.set(true); }
                                running.set(true);
                            },
                            if legacy { "Enter legacy images (reboot required)" } else { "Start" }
                        }
                    }
                }
                if needs_reboot() {
                    p { role: "alert", class: "text-sm text-amber-700 dark:text-amber-300",
                        "Tracking has not been restored. Reboot then wait for a new USB connection before using tracking/settings. If reboot is unavailable or recovery fails, fully power-cycle the device (disconnecting USB alone may leave a battery-powered unit on)."
                    }
                    button {
                        class: BUTTON,
                        disabled: rebooting() || !device.capabilities.contains(DeviceCapabilities::CONTROL),
                        onclick: move |_| {
                            running.set(false);
                            rebooting.set(true);
                            status.set("Stopping pixels and requesting legacy reboot; waiting for reconnect is required.".to_owned());
                            let device = reboot_device.clone();
                            let client = hub.peek().client();
                            spawn(async move {
                                let result = match client {
                                    Some(client) => client.device(device).reboot_device().await.map_err(|cause| cause.to_string()),
                                    None => Err("not connected to the hub".to_owned()),
                                };
                                match result {
                                    Ok(()) => status.set("Reboot acknowledged. Wait for the device to reconnect; acknowledgement alone does not confirm tracking recovery.".to_owned()),
                                    Err(cause) => error.set(Some(format!("Reboot failed: {cause}. Fully power-cycle the unit and reconnect."))),
                                }
                                rebooting.set(false);
                            });
                        },
                        "Stop images & reboot legacy device"
                    }
                }
                p { role: "status", class: HELP, "{status}" }
                if let Some(message) = error() {
                    p { role: "alert", class: "text-sm text-red-700 dark:text-red-300", {message} }
                }
                if running() && can_stream {
                    LiveSession { hub, device, camera: camera(), fps: fps(), latest, status, error, running }
                }
                if latest.read().is_some() {
                    FrameView { latest, now: now(), live: running() && can_stream }
                } else {
                    p { class: HELP, "No image received." }
                }
            }
        }
    }
}
