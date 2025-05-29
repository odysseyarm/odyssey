use dioxus::prelude::*;
use crate::components::crosshair_manager::CrosshairManager;
use crate::hub;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
pub fn Zero(hub: Signal<hub::HubContext>) -> Element {
    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        div {
            class: "flex h-screen bg-gray-50 dark:bg-gray-800",

            aside {
                class: "fixed top-0 left-0 z-40 h-screen transition-transform -translate-x-full sm:translate-x-0",
                div {
                    class: "h-full px-3 py-4 overflow-y-auto bg-gray-100 dark:bg-gray-900",
                    ul {
                        class: "space-y-2 font-medium",
                        li {
                            button {
                                class: "py-2.5 px-5 ms-3 text-base text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-gray-600 dark:hover:text-white dark:hover:bg-gray-700",
                                "Reset"
                            }
                        }
                        li {
                            button {
                                class: "ms-3 py-2.5 px-5 text-base text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-gray-600 dark:hover:text-white dark:hover:bg-gray-700",
                                "Clear"
                            }
                        }
                    }
                }
            }

            div {
                class: "flex-1 flex items-center justify-center max-h-full max-w-full",
                CrosshairManager { hub },
                img {
                    class: "w-96",
                    src: asset!("/assets/images/target.avif"),
                    alt: "Zero Target"
                }
            }
        }
    }
}
