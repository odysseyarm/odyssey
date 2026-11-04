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

// The sidebar is gray-100, so hover and the current page sit on white (or the
// darker gray in dark mode) to stand out from it.
const LINK_CLASS: &str = "flex items-center px-3 py-2 rounded-lg text-gray-700 dark:text-gray-300 hover:bg-white hover:text-gray-900 dark:hover:bg-gray-800 dark:hover:text-white focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500";
const ACTIVE_CLASS: &str =
    "bg-white text-gray-900 font-semibold shadow-sm dark:bg-gray-800 dark:text-white";

#[component]
pub fn Navbar() -> Element {
    rsx! {
        div {
            class: "flex",
            aside {
                class: "fixed top-0 left-0 z-40 w-36 h-screen bg-gray-100 dark:bg-gray-900 border-r border-gray-200 dark:border-gray-800",
                nav {
                    class: "h-full px-3 py-4 overflow-y-auto",
                    aria_label: "Main",
                    div {
                        class: "px-3 pb-4 text-lg font-bold tracking-tight text-gray-900 dark:text-white",
                        "Odyssey"
                    }
                    ul {
                        class: "space-y-1 text-sm font-medium",
                        for (label, route) in PAGES {
                            li {
                                key: "{label}",
                                Link {
                                    to: route,
                                    class: LINK_CLASS,
                                    active_class: ACTIVE_CLASS,
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
