// src/components/zero.rs

use crate::components::crosshair_manager::CrosshairManager;
use crate::hub;
use dioxus::{html::geometry::euclid::Rect, logger::tracing, prelude::*};
use odyssey_hub_common::events as oe;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
pub fn Zero(hub: Signal<hub::HubContext>) -> Element {
    let devices = (hub().devices)();

    // one Signal<bool> per slot: true means “zero on next shot”
    let mut shooting_devices = use_signal(|| Vec::<Signal<bool>>::new());

    // ensure we can safely index [0 .. devices.len())
    {
        let mut w = shooting_devices.write();
        if w.len() < devices.len() {
            w.resize_with(devices.len(), || Signal::new(false));
        }
    }

    // for positioning the crosshair overlay
    let mut crosshair_manager_div = use_signal(|| None);
    let mut rect_signal = use_signal(Rect::<f32, _>::zero);
    let window = dioxus::desktop::use_window();

    // memoized ratio of center to window for zero()
    let zero_screen_ratio = use_memo(move || {
        let ws = window.inner_size().to_logical::<f32>(window.scale_factor());
        let center = rect_signal.read().center();
        (center.x / ws.width, center.y / ws.height)
    });
    tracing::info!("Zero screen ratio: {:?}", zero_screen_ratio());

    // fire off zero() when we see an ImpactEvent and the flag is set
    use_effect(move || {
        let ctx = hub();
        if let Some(event) = (ctx.latest_event)() {
            match event {
                oe::Event::DeviceEvent(oe::DeviceEvent(
                    device,
                    oe::DeviceEventKind::ImpactEvent { .. },
                )) => {
                    if let Some(key) = ctx.device_key(&device) {
                        if shooting_devices.peek()[key]() {
                            let zr = *zero_screen_ratio.peek();
                            let dev = device.clone();
                            spawn(async move {
                                if let Err(e) = (ctx.client)()
                                    .zero(
                                        dev.clone(),
                                        nalgebra::Vector3::new(0., -0.0635, 0.).into(),
                                        nalgebra::Vector2::new(zr.0, zr.1).into(),
                                    )
                                    .await
                                {
                                    tracing::error!(
                                        "Failed to zero device {:x}: {}",
                                        dev.uuid(),
                                        e
                                    );
                                }
                            });
                            shooting_devices.write()[key].set(false);
                        }
                    }
                }
                oe::Event::DeviceEvent(oe::DeviceEvent(
                    device,
                    oe::DeviceEventKind::DisconnectEvent,
                )) => {
                    if let Some(key) = hub.peek().device_key(&device) {
                        shooting_devices.write()[key].set(false);
                    }
                }
                _ => {}
            }
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
                    ul { class: "space-y-2 font-medium",
                        for (slot, device) in devices {
                            {
                                let firing = shooting_devices()[slot]();
                                rsx! {
                                    li {
                                        class: "flex flex-col",
                                        span {
                                            class: "text-gray-900 dark:text-white",
                                            {format!("0x{:x}", device.uuid())},
                                        }
                                        ul { class: "flex justify-start",
                                            li { class: "flex items-center",
                                                if firing {
                                                    button {
                                                        class: "py-2.5 px-5 me-3 text-xs text-gray-900 focus:outline-none bg-white rounded-lg border border-red-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-red-600 dark:hover:text-white dark:hover:bg-gray-700",
                                                        onclick: move |_| {
                                                            shooting_devices.write()[slot].set(false);
                                                        },
                                                        "Cancel"
                                                    }
                                                } else {
                                                    button {
                                                        class: "py-2.5 px-5 me-3 text-xs text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-gray-600 dark:hover:text-white dark:hover:bg-gray-700",
                                                        onclick: move |_| {
                                                            shooting_devices.write()[slot].set(true);
                                                        },
                                                        "Zero on shot"
                                                    }
                                                }
                                                button { class: "py-2.5 px-5 me-3 text-xs text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-gray-600 dark:hover:text-white dark:hover:bg-gray-700", onclick: {
                                                    let dev = device.clone();
                                                    move |_| {
                                                        let dev = dev.clone();
                                                        async move {
                                                            match (hub().client)().reset_zero(dev.clone()).await {
                                                                Ok(_) => tracing::info!("Cleared zero for {:x}", dev.uuid()),
                                                                Err(e) => tracing::error!("Failed to reset zero {:x}: {}", dev.uuid(), e),
                                                            }
                                                        }
                                                    }
                                                }, "Reset Zero" }
                                                button { class: "py-2.5 px-5 me-3 text-xs text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-gray-600 dark:hover:text-white dark:hover:bg-gray-700", onclick: {
                                                    let dev = device.clone();
                                                    move |_| {
                                                        let dev = dev.clone();
                                                        async move {
                                                            match (hub().client)().clear_zero(dev.clone()).await {
                                                                Ok(_) => tracing::info!("Cleared zero for {:x}", dev.uuid()),
                                                                Err(e) => tracing::error!("Failed to clear zero {:x}: {}", dev.uuid(), e),
                                                            }
                                                        }
                                                    }
                                                }, "Clear Zero" }
                                                button { class: "py-2.5 px-5 me-3 text-xs text-gray-900 focus:outline-none bg-white rounded-lg border border-gray-200 hover:bg-gray-100 hover:text-blue-700 focus:z-10 focus:ring-4 focus:ring-gray-100 dark:focus:ring-gray-700 dark:bg-gray-800 dark:text-gray-400 dark:border-gray-600 dark:hover:text-white dark:hover:bg-gray-700", onclick: {
                                                    let dev = device.clone();
                                                    move |_| {
                                                        let dev = dev.clone();
                                                        async move {
                                                            match (hub().client)().save_zero(dev.clone()).await {
                                                                Ok(_) => tracing::info!("Saved zero for {:x}", dev.uuid()),
                                                                Err(e) => tracing::error!("Failed to save zero {:x}: {}", dev.uuid(), e),
                                                            }
                                                        }
                                                    }
                                                }, "Save Zero" }
                                            }
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
                    format!("url({})", asset!("/assets/images/target.avif"))
                ),
                onmounted: move |cx| async move {
                    if let Ok(r) = cx.data().as_ref().get_client_rect().await {
                        rect_signal.set(r.cast());
                    }
                    crosshair_manager_div.set(Some(cx.data()));
                },
                onresize: move |_| async move {
                    if let Some(root) = crosshair_manager_div() {
                        if let Ok(r) = root.as_ref().get_client_rect().await {
                            rect_signal.set(r.cast());
                        }
                    }
                },
                CrosshairManager { hub },
            }

            button {
                class: "fixed z-50 top-4 right-4 …",
                onclick: move |_| dioxus::desktop::window().close(),
                "Close"
            }
        }
    }
}
