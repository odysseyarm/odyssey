//! The Zero window: a fullscreen target on the chosen monitor and a sidebar to
//! arm each device's zero.
//!
//! The window runs in its own VirtualDom, so nothing here can use the main
//! window's context; the hub and the tracking sender arrive as props and events
//! come over broadcast channels.

mod device_controls;
mod device_signals;
mod device_sync;
mod history;
mod save_zero_button;
mod shot_delay_control;
mod zeroing;

use device_controls::DeviceControls;
use device_signals::SignalsByDevice;
use device_sync::use_device_signal_sync;
use dioxus::prelude::*;
use history::use_tracking_history;
use zeroing::{use_zero_on_impact, zero_on_key};

use crate::components::Theme;
use crate::components::crosshair_manager::{CrosshairManager, TrackingSender};
use crate::hub::HubContext;

/// Zero point in screen space. The target layer covers the window, the window
/// is fullscreen on the screen the hub resolves against, and the target image
/// is centered in that layer, so the target sits at the screen center.
const ZERO_TARGET: nalgebra::Vector2<f32> = nalgebra::Vector2::new(0.5, 0.5);

#[component]
pub(crate) fn Zero(
    hub: Signal<HubContext>,
    /// A plain sender rather than a `Signal`, because this runs in its own VirtualDom.
    tracking_sender: TrackingSender,
) -> Element {
    let devices = (hub().devices)();

    // Per-device zeroing state, keyed by uuid. Slab slots are reused as devices
    // come and go, so keying by slot hands one device the previous occupant's
    // "zero on shot" flag and shot delay.
    let mut device_signals = use_signal(SignalsByDevice::new);
    {
        let mut registered = device_signals.write();
        for (_, device) in &devices {
            registered.entry(device.uuid).or_default();
        }
    }

    let tracking_history = use_tracking_history(hub);
    use_zero_on_impact(hub, device_signals, tracking_history);
    use_device_signal_sync(hub, device_signals);

    rsx! {
        Theme {}

        div {
            class: "flex h-screen bg-canvas",
            tabindex: "0",
            autofocus: true,
            onkeydown: move |e: KeyboardEvent| {
                zero_on_key(hub, device_signals, tracking_history, e.key().to_string());
            },

            aside {
                class: "fixed top-0 left-0 z-40 h-screen transition-transform -translate-x-full sm:translate-x-0",
                div {
                    class: "h-full px-3 py-4 overflow-y-auto bg-surface/80",
                    ul { class: "space-y-2 font-medium",
                        for (slot, device) in devices {
                            if let Some(signals) = device_signals.peek().get(&device.uuid).copied() {
                                DeviceControls { key: "{device.uuid:?}", hub, slot, device, signals }
                            }
                        }
                    }
                }
            }

            // Pinned to the viewport, not laid out next to the sidebar: the
            // target center has to be the screen center, and CrosshairManager
            // maps aimpoints onto this same box.
            div {
                class: "fixed inset-0 flex items-center justify-center bg-center bg-no-repeat",
                style: format!(
                    "background-image: url({}); background-size: clamp(0in, 11in, 40%) auto;",
                    asset!("/assets/images/target.avif")
                ),
                CrosshairManager { hub, tracking_sender: Some(tracking_sender) },
            }

            button {
                class: "fixed z-50 top-4 right-4 w-10 h-10 rounded-full bg-black/60 hover:bg-black/80 text-white text-xl flex items-center justify-center transition-colors",
                onclick: move |_| dioxus::desktop::window().close(),
                "✕"
            }
        }
    }
}
