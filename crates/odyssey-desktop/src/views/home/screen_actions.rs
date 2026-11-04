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

const CALIBRATE_LINK: &str = "shrink-0 text-gray-900 bg-white border border-gray-300 hover:bg-gray-100 focus:ring-4 focus:ring-gray-200 font-medium rounded-lg text-base px-5 py-2.5 focus:outline-none dark:bg-gray-800 dark:text-white dark:border-gray-600 dark:hover:bg-gray-700 dark:focus:ring-gray-700";
const ZERO_BUTTON: &str = "shrink-0 text-white bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 font-medium rounded-lg text-base px-5 py-2.5 dark:bg-blue-600 dark:hover:bg-blue-700 focus:outline-none dark:focus:ring-blue-800 disabled:opacity-50 disabled:pointer-events-none";

#[component]
pub(super) fn ScreenActions(hub: Signal<HubContext>) -> Element {
    let monitor_selection = use_monitor_selection();
    rsx! {
        div {
            class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow flex flex-col gap-3",

            ScreenPicker { selection: monitor_selection }

            div {
                class: "flex flex-row justify-end gap-3",
                Link {
                    to: Route::Calibrate {},
                    class: CALIBRATE_LINK,
                    "Calibrate screen"
                }
                button {
                    class: ZERO_BUTTON,
                    r#type: "button",
                    disabled: monitor_selection.selected().is_none(),
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
