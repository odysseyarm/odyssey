use dioxus::prelude::*;
use dioxus_router::prelude::*;
use crate::Route;

#[component]
pub fn Navbar() -> Element {
    rsx! {
        div {
            class: "flex",
            // Sidebar
            aside {
                class: "fixed top-0 left-0 z-40 w-32 h-screen transition-transform sm:translate-x-0 bg-gray-50 dark:bg-gray-900",
                nav {
                    class: "h-full px-3 py-4 overflow-y-auto",
                    ul {
                        class: "space-y-2 font-medium",
                        li {
                            Link {
                                to: Route::Home {},
                                class: "flex items-center p-2 text-gray-900 rounded-lg dark:text-white hover:bg-gray-100 dark:hover:bg-gray-700 group",
                                span {
                                    class: "ms-3",
                                    "Home"
                                }
                            }
                        }
                    }
                }
            }

            main {
                class: "flex-1 sm:ml-32",
                Outlet::<Route> {}
            }
        }
    }
}
