//! Choosing the screen, then calibrating it or zeroing on it.

use dioxus::desktop::tao::monitor::MonitorHandle;
use dioxus::desktop::tao::window::Fullscreen;
use dioxus::desktop::{Config, WindowBuilder, WindowCloseBehaviour};
use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;
use crate::components::crosshair_manager::TrackingSender;
use crate::components::{ScreenPicker, use_monitor_selection};
use crate::hub::HubContext;
use crate::views::zero::{Zero, ZeroProps};

#[component]
pub(super) fn ScreenActions(hub: Signal<HubContext>) -> Element {
    let monitor_selection = use_monitor_selection();
    let no_devices = (hub().devices)().is_empty();
    rsx! {
        div {
            class: "card flex flex-col gap-3",

            ScreenPicker { selection: monitor_selection }

            div {
                class: "flex flex-row justify-end gap-3",
                Link {
                    to: Route::Calibrate {},
                    class: "btn btn-secondary shrink-0",
                    "Calibrate screen"
                }
                button {
                    class: "btn btn-primary shrink-0",
                    r#type: "button",
                    disabled: no_devices || monitor_selection.selected().is_none(),
                    onclick: move |_| {
                        if let Some(monitor) = monitor_selection.selected_handle() {
                            open_zero_window(hub, monitor);
                        }
                    },
                    "Zero"
                }
            }
        }
    }
}

/// Opens the Zero window fullscreen on `monitor`.
fn open_zero_window(hub: Signal<HubContext>, monitor: MonitorHandle) {
    let tracking_sender = TrackingSender(hub.peek().tracking_events.clone());
    let dom = VirtualDom::new_with_props(
        Zero,
        ZeroProps {
            hub,
            tracking_sender,
        },
    );
    let config = Config::default()
        .with_menu(None)
        .with_close_behaviour(WindowCloseBehaviour::WindowCloses)
        .with_window(
            WindowBuilder::new()
                .with_position(monitor.position())
                .with_fullscreen(Some(Fullscreen::Borderless(Some(monitor)))),
        );
    dioxus::desktop::window().new_window(dom, config);
}
