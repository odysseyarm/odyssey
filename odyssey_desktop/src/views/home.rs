use dioxus::{
    desktop::{tao::window::Fullscreen, WindowBuilder},
    prelude::*,
};

use crate::hub;

#[component]
pub fn Home() -> Element {
    let hub = use_context::<Signal<hub::HubContext>>();

    let window = dioxus::desktop::use_window();
    let options = use_memo(move || window.available_monitors().collect::<Vec<_>>());

    let mut selected_index = use_signal(|| 0_usize);
    let selected_value = use_memo(move || options().get(selected_index()).cloned());

    rsx! {
        div {
            class: "min-h-screen flex flex-col items-center justify-center bg-gray-50 dark:bg-gray-800",

            div {
                class: "w-full max-w-md flex flex-col items-stretch gap-4",

                h1 {
                    class: "text-center text-2xl font-bold text-gray-900 dark:text-white",
                    "Welcome!"
                }

                form {
                    class: "flex flex-col text-gray-500 dark:text-gray-400 gap-4",

                    label {
                        class: "text-base font-medium",
                        "Choose a display:"
                    }

                    div {
                        class: "flex flex-row items-center gap-2",

                        select {
                            class: "bg-gray-50 border border-gray-300 text-gray-900 text-base rounded-lg focus:ring-blue-500 focus:border-blue-500 block w-full p-2.5 dark:bg-gray-700 dark:border-gray-600 dark:placeholder-gray-400 dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500",
                            onchange: move |evt| {
                                if let Ok(index) = evt.value().parse::<usize>() {
                                    if index < options().len() {
                                        selected_index.set(index);
                                    }
                                }
                            },
                            option { value: "", disabled: true, selected: selected_value().is_none(), "Select an option" },
                            for (i, display) in options().iter().enumerate() {
                                option {
                                    value: "{i}",
                                    "{display.name().unwrap_or(\"Unknown Display\".to_string())}",
                                }
                            }
                        }

                        button {
                            class: "w-12 shrink-0 text-white bg-blue-700 hover:bg-blue-800 focus:ring-4 focus:ring-blue-300 font-medium rounded-lg text-base p-2.5 dark:bg-blue-600 dark:hover:bg-blue-700 focus:outline-none dark:focus:ring-blue-800",
                            r#type: "button",
                            onclick: move |_| {
                                // why is async necessary?
                                async move {
                                    if let Some(sel) = selected_value() {
                                        let dom = VirtualDom::new_with_props(crate::views::Zero, crate::views::zero::ZeroProps { hub });
                                        // HACK: with_position is necessary for reading the correct scale factor off the window
                                        let config = dioxus::desktop::Config::default().with_menu(None).with_close_behaviour(dioxus::desktop::WindowCloseBehaviour::WindowCloses).with_window(WindowBuilder::new().with_position(sel.position()).with_fullscreen(Some(Fullscreen::Borderless(Some(sel)))));
                                        dioxus::desktop::window().new_window(dom, config);
                                    }
                                }
                            },
                            "Go"
                        }
                    }
                }
            }
        }
    }
}
