use crate::components::crosshair_manager::CrosshairManager;
use crate::hub;
use dioxus::{html::geometry::euclid::Rect, prelude::*};
use odyssey_hub_common::events as oe;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
pub fn Zero(hub: Signal<hub::HubContext>) -> Element {
    let devices = (hub().devices)();

    let mut crosshair_manager_div = use_signal(|| None);
    let mut rect_signal = use_signal(Rect::<f32, _>::zero);
    let window = dioxus::desktop::use_window();

    let zero_screen_ratio = use_memo(move || {
            let window_size = window.inner_size().to_logical::<f32>(window.scale_factor());
            let center = rect_signal.read().center();
            let (win_w, win_h) = (window_size.width, window_size.height);
            (center.x / win_w, center.y / win_h)
        }
    );

    dioxus::logger::tracing::info!("Zero screen ratio: {:?}", zero_screen_ratio());

    let mut shooting_devices = use_signal(|| Vec::<Signal<bool>>::new());

    fn creative_get(vec: &mut Vec<Signal<bool>>, index: usize) -> bool {
        if index >= vec.len() {
            vec.resize(index + 1, Signal::new(false));
        }
        vec[index]()
    }

    fn creative_write(vec: &mut Vec<Signal<bool>>, index: usize, value: bool) {
        if index >= vec.len() {
            vec.resize(index + 1, Signal::new(false));
        }
        vec[index].set(value);
    }

    use_effect(move || {
        let hub_snapshot = hub();
        match (hub_snapshot.latest_event)() {
            Some(oe::Event::DeviceEvent(oe::DeviceEvent(
                device,
                oe::DeviceEventKind::ImpactEvent(oe::ImpactEvent { timestamp: _ }),
            ))) => {
                let device_key = hub_snapshot.device_key(&device);
                if let Some(key) = device_key {
                    if creative_get(&mut shooting_devices.write(), key) {
                        spawn(async move {
                            match (hub_snapshot.client)()
                                .zero(
                                    device.clone(),
                                    nalgebra::Vector3::<f32>::new(0., -0.0635, 0.).into(),
                                    {
                                        let (a, b) = *zero_screen_ratio.peek();
                                        nalgebra::Vector2::new(a, b).into()
                                    },
                                )
                                .await
                            {
                                Ok(_) => {}
                                Err(e) => {
                                    dioxus::logger::tracing::error!(
                                        "Failed to zero device {:x}: {}",
                                        device.uuid(),
                                        e
                                    );
                                }
                            }
                        });
                        creative_write(&mut shooting_devices.write(), key, false);
                    }
                }
            }
            Some(oe::Event::DeviceEvent(oe::DeviceEvent(
                device,
                oe::DeviceEventKind::DisconnectEvent,
            ))) => {
                if let Some(key) = hub.peek().device_key(&device) {
                    creative_write(&mut shooting_devices.write(), key, false);
                }
            }
            _ => {}
        }
    });

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        div {
            class: "flex h-screen bg-gray-50 dark:bg-gray-800",

            aside {
                class: "fixed top-0 left-0 z-40 h-screen transition-transform -translate-x-full sm:translate-x-0",
                div {
                    class: "h-full px-3 py-4 overflow-y-auto bg-gray-100/5 dark:bg-gray-900/5",
                    ul {
                        class: "space-y-2 font-medium",
                        // li {
                        //     button {
                        //         class: "py-2.5 px-5 ms-3 text-base text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-gray-600 dark:hover:text-white dark:hover:bg-gray-700",
                        //         "Reset All"
                        //     }
                        // }
                        // li {
                        //     button {
                        //         class: "ms-3 py-2.5 px-5 text-base text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-gray-600 dark:hover:text-white dark:hover:bg-gray-700",
                        //         "Clear All"
                        //     }
                        // }
                        for (slot, device) in devices {
                            li {
                                class: "flex flex-col",
                                span {
                                    class: "text-gray-900 dark:text-white",
                                    {format!("0x{:x}", device.uuid())},
                                }
                                ul {
                                    class: "flex justify-start",
                                    li {
                                        class: "flex items-center",
                                        if creative_get(&mut shooting_devices.write(), slot) {
                                            button {
                                                class: "py-2.5 px-5 me-3 text-xs text-gray-900 focus:outline-none bg-white rounded-lg border border-red-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-red-600 dark:hover:text-white dark:hover:bg-gray-700",
                                                onclick: move |_| {
                                                    creative_write(&mut shooting_devices.write(), slot, false);
                                                },
                                                "Cancel"
                                            }
                                        } else {
                                            button {
                                                class: "py-2.5 px-5 me-3 text-xs text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-gray-600 dark:hover:text-white dark:hover:bg-gray-700",
                                                onclick: move |_| {
                                                    creative_write(&mut shooting_devices.write(), slot, true);
                                                },
                                                "Zero on shot"
                                            }
                                        }
                                        button {
                                            class: "py-2.5 px-5 me-3 text-xs text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-gray-600 dark:hover:text-white dark:hover:bg-gray-700",
                                            onclick: {
                                                let device = device.clone();
                                                move |_| {
                                                    let device = device.clone();
                                                    async move {
                                                        match (hub().client)().reset_zero(device.clone()).await {
                                                            Ok(_) => {
                                                                dioxus::logger::tracing::info!("Cleared zero for device {:x}", device.uuid());
                                                            }
                                                            Err(e) => {
                                                                dioxus::logger::tracing::error!("Failed to reset zero for device {:x}: {}", device.uuid(), e);
                                                            }
                                                        }
                                                    }
                                                }
                                            },
                                            "Reset Zero"
                                        }
                                        button {
                                            class: "py-2.5 px-5 me-3 text-xs text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-gray-600 dark:hover:text-white dark:hover:bg-gray-700",
                                            onclick: {
                                                let device = device.clone();
                                                move |_| {
                                                    let device = device.clone();
                                                    async move {
                                                        match (hub().client)().clear_zero(device.clone()).await {
                                                            Ok(_) => {
                                                                dioxus::logger::tracing::info!("Cleared zero for device {:x}", device.uuid());
                                                            }
                                                            Err(e) => {
                                                                dioxus::logger::tracing::error!("Failed to clear zero for device {:x}: {}", device.uuid(), e);
                                                            }
                                                        }
                                                    }
                                                }
                                            },
                                            "Clear Zero"
                                        }
                                        button {
                                            class: "py-2.5 px-5 me-3 text-xs text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-gray-600 dark:hover:text-white dark:hover:bg-gray-700",
                                            onclick: {
                                                let device = device.clone();
                                                move |_| {
                                                    let device = device.clone();
                                                    async move {
                                                        match (hub().client)().save_zero(device.clone()).await {
                                                            Ok(_) => {
                                                                dioxus::logger::tracing::info!("Saved zero for device {:x}", device.uuid());
                                                            }
                                                            Err(e) => {
                                                                dioxus::logger::tracing::error!("Failed to save zero for device {:x}: {}", device.uuid(), e);
                                                            }
                                                        }
                                                    }
                                                }
                                            },
                                            "Save Zero"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div {
                class: "flex-1 flex items-center justify-center h-full w-full bg-center bg-no-repeat",
                style: format!(
                    "background-image: {}; background-size: clamp(0in, 11in, 40%) auto;",
                    format!("url({})", asset!("/assets/images/target.avif")),
                ),
                onmounted: move |cx| async move {
                    let cx_data = cx.data();
                    let client_rect = cx_data.as_ref().get_client_rect();
                    if let Ok(rect) = client_rect.await {
                        rect_signal.set(rect.cast());
                    }
                    crosshair_manager_div.set(Some(cx_data));
                },
                onresize: move |_| async move {
                    if let Some(crosshair_manager_div) = crosshair_manager_div() {
                        let client_rect = crosshair_manager_div.as_ref().get_client_rect();
                        if let Ok(rect) = client_rect.await {
                            rect_signal.set(rect.cast());
                        }
                    }
                },
                CrosshairManager { hub },
            }
            button {
                class: "fixed z-50 top-4 right-4 text-white bg-red-700 hover:bg-red-800 focus:outline-none focus:ring-4 focus:ring-red-300 font-medium rounded-full text-sm px-5 py-2.5 text-center dark:bg-red-600 dark:hover:bg-red-700 dark:focus:ring-red-900",
                onclick: move |_| {
                    dioxus::desktop::window().close();
                },
                "Close"
            },
        }
    }
}
