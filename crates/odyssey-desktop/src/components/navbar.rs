use dioxus::prelude::*;
use dioxus_router::{Link, Outlet};

use crate::Route;

/// Every page in the sidebar, in order.
const PAGES: [(&str, Route); 5] = [
    ("Home", Route::Home {}),
    ("Pairing", Route::Pairing {}),
    ("Accessories", Route::Accessories {}),
    ("Calibrate", Route::Calibrate {}),
    ("Settings", Route::Settings {}),
];

#[component]
pub fn Navbar() -> Element {
    rsx! {
        div {
            class: "flex",
            aside {
                class: "fixed top-0 left-0 z-40 w-36 h-screen bg-surface-alt border-r border-line",
                nav {
                    class: "h-full px-3 py-4 overflow-y-auto",
                    aria_label: "Main",
                    div {
                        class: "px-3 pb-4 text-lg font-bold tracking-tight text-accent-ink",
                        "Odyssey"
                    }
                    ul {
                        class: "space-y-1 text-sm font-medium",
                        for (label, route) in PAGES {
                            li {
                                key: "{label}",
                                Link {
                                    to: route,
                                    // The current page takes the accent tint; hover is a neutral lift.
                                    class: "flex items-center px-3 py-2 rounded-lg text-fg-muted hover:bg-surface-hover hover:text-fg-strong",
                                    active_class: "bg-accent-soft text-accent-ink font-semibold",
                                    {label}
                                }
                            }
                        }
                    }
                }
            }

            main {
                class: "flex-1 ml-36 min-w-0",
                Outlet::<Route> {}
            }
        }
    }
}
