//! Calibrate page: the ways to calibrate a screen. Each method is a wizard
//! with its own route; this page lists them. A new method adds a module
//! beside [`automatic`], a route, and one entry in [`METHODS`].

pub mod automatic;

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::components::wizard::{Icon, IconView};
use crate::Route;

/// A way to calibrate a screen.
struct Method {
    title: &'static str,
    description: &'static str,
    icon: Icon,
    route: fn() -> Route,
}

const METHODS: [Method; 1] = [Method {
    title: "Automatic (camera)",
    description: "A depth camera looks at the screen and measures its size, position and the tracker markers. Takes a couple of minutes.",
    icon: Icon::Camera,
    route: || Route::CalibrateAutomatic {},
}];

#[component]
pub fn Calibrate() -> Element {
    rsx! {
        div {
            class: "min-h-screen bg-gray-50 p-6 dark:bg-gray-800",
            div {
                class: "mx-auto flex max-w-2xl flex-col gap-4",
                h1 { class: "text-2xl font-bold text-gray-900 dark:text-white", "Calibrate a screen" }
                p {
                    class: "text-base text-gray-600 dark:text-gray-300",
                    "Calibrating teaches the system where your screen is, so shots land where you aim. Choose how to do it."
                }
                for method in METHODS.iter() {
                    Link {
                        key: "{method.title}",
                        to: (method.route)(),
                        class: "flex items-center gap-4 rounded-xl bg-white p-5 shadow hover:ring-4 hover:ring-blue-200 focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:bg-gray-700 dark:hover:ring-blue-900",
                        IconView { icon: method.icon, class: "size-10 shrink-0 text-blue-600 dark:text-blue-400".to_string() }
                        div {
                            div { class: "text-xl font-semibold text-gray-900 dark:text-white", "{method.title}" }
                            div { class: "text-base text-gray-600 dark:text-gray-300", "{method.description}" }
                        }
                    }
                }
            }
        }
    }
}
