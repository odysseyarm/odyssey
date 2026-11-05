//! Calibrate page: the ways to calibrate a screen. Each method is a wizard
//! with its own route; this page lists them. A new method adds a module
//! beside [`automatic`], a route, and one entry in [`METHODS`].

pub(crate) mod automatic;

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;
use crate::components::Page;
use crate::components::wizard::{Icon, IconView};

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
        Page {
            title: "Calibrate a screen",
            description: "Calibrating teaches the system where your screen is, so shots land where you aim. Choose how to do it.",
            for method in METHODS.iter() {
                Link {
                    key: "{method.title}",
                    to: (method.route)(),
                    class: "card flex items-center gap-4 p-5 transition-colors hover:border-accent-ink",
                    IconView { icon: method.icon, class: "size-10 shrink-0 text-accent-ink".to_string() }
                    div {
                        div { class: "text-lg font-semibold text-fg-strong", "{method.title}" }
                        div { class: "text-sm text-fg-muted", "{method.description}" }
                    }
                }
            }
        }
    }
}
