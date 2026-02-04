use crate::components::crosshair_manager::{CrosshairManager, TrackingSender};
use crate::hub;
use dioxus::{html::geometry::euclid::Rect, logger::tracing, prelude::*};
use odyssey_hub_common::events as oe;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[derive(Default)]
struct DeviceSignals {
    shooting: Signal<bool>,
    shot_delay: Signal<u16>,
}

#[component]
pub fn Zero(
    hub: Signal<hub::HubContext>,
    /// Pass the tracking sender directly to work across VirtualDoms
    tracking_sender: TrackingSender,
) -> Element {
    let devices = (hub().devices)();

    // one Signal<bool> per slot: true means “zero on next shot”
    let mut device_signals = use_signal(|| Vec::<DeviceSignals>::new());
    let last_device_snapshot = use_signal(|| Vec::<[u8; 6]>::new());

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
    // tracing::info!("Zero screen ratio: {:?}", zero_screen_ratio());

    // ensure we can safely index [0 .. devices.len())
    {
        let mut w = device_signals.write();
        if w.len() < devices.len() {
            w.resize_with(devices.len(), || Default::default());
        }
    }

    // fire off zero() when we see an ImpactEvent and the flag is set
    use_effect(move || {
        let ctx = hub();
        if let Some(event) = (ctx.latest_event)() {
            match event {
                oe::Event::DeviceEvent(oe::DeviceEvent(device, kind)) => match kind {
                    oe::DeviceEventKind::ImpactEvent { .. } => {
                        println!("ImpactEvent for device: {:?}", device);
                        if let Some(key) = ctx.device_key(&device) {
                            if *device_signals.peek()[key].shooting.peek() {
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
                                            "Failed to zero device {:x?}: {}",
                                            dev.uuid,
                                            e
                                        );
                                    }
                                });
                                device_signals.write()[key].shooting.set(false);
                            }
                        }
                    }
                    _ => {}
                },
            }
        }
    });

    {
        let hub = hub.clone();
        let mut device_signals = device_signals.clone();
        let mut last_device_snapshot = last_device_snapshot.clone();
        let device_entries: Vec<(usize, odyssey_hub_common::device::Device)> = devices
            .iter()
            .map(|(slot, dev)| (slot, dev.clone()))
            .collect();
        let snapshot: Vec<[u8; 6]> = device_entries.iter().map(|(_, dev)| dev.uuid).collect();

        use_effect(move || {
            if last_device_snapshot.peek().as_slice() == snapshot.as_slice() {
                return ();
            }
            last_device_snapshot.set(snapshot.clone());

            spawn({
                let device_entries = device_entries.clone();
                async move {
                    {
                        let mut w = device_signals.write();
                        if w.len() > device_entries.len() {
                            w.truncate(device_entries.len());
                        } else if w.len() < device_entries.len() {
                            w.resize_with(device_entries.len(), || Default::default());
                        }
                    }

                    for (slot, device) in device_entries {
                        let hub = hub.clone();
                        let mut device_signals = device_signals.clone();
                        spawn(async move {
                            match hub
                                .peek()
                                .client
                                .peek()
                                .clone()
                                .get_shot_delay(device.clone())
                                .await
                            {
                                Ok(delay) => {
                                    device_signals.write()[slot].shot_delay.set(delay);
                                }
                                Err(e) => tracing::error!("Failed to get shot delay: {e}"),
                            }
                        });
                    }
                }
            });

            ()
        });
    }

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
                                let firing = (device_signals.read()[slot].shooting)();
                                rsx! {
                                    li {
                                        class: "flex flex-col gap-2",
                                        span {
                                            class: "text-gray-900 dark:text-white",
                                            {format!("0x{:x?}", device.uuid)},
                                        }
                                        ul { class: "flex flex-col gap-2",
                                            li { class: "flex items-center gap-3",
                                                if firing {
                                                    button {
                                                        class: "btn-secondary-danger",
                                                        onclick: move |_| {
                                                            device_signals.write()[slot].shooting.set(false);
                                                        },
                                                        "Cancel"
                                                    }
                                                } else {
                                                    button {
                                                        class: "btn-secondary",
                                                        onclick: move |_| {
                                                            device_signals.write()[slot].shooting.set(true);
                                                        },
                                                        "Zero on shot"
                                                    }
                                                }
                                                button { class: "btn-secondary", onclick: {
                                                    let dev = device.clone();
                                                    move |_| {
                                                        let dev = dev.clone();
                                                        async move {
                                                            match (hub().client)().reset_zero(dev.clone()).await {
                                                                Ok(_) => tracing::info!("Cleared zero for {:x?}", dev.uuid),
                                                                Err(e) => tracing::error!("Failed to reset zero {:x?}: {}", dev.uuid, e),
                                                            }
                                                        }
                                                    }
                                                }, "Reset Zero" }
                                                button { class: "btn-secondary", onclick: {
                                                    let dev = device.clone();
                                                    move |_| {
                                                        let dev = dev.clone();
                                                        async move {
                                                            match (hub().client)().clear_zero(dev.clone()).await {
                                                                Ok(_) => tracing::info!("Cleared zero for {:x?}", dev.uuid),
                                                                Err(e) => tracing::error!("Failed to clear zero {:x?}: {}", dev.uuid, e),
                                                            }
                                                        }
                                                    }
                                                }, "Clear Zero" }
                                                button { class: "btn-secondary", onclick: {
                                                    let dev = device.clone();
                                                    move |_| {
                                                        let dev = dev.clone();
                                                        async move {
                                                            match (hub().client)().save_zero(dev.clone()).await {
                                                                Ok(_) => tracing::info!("Saved zero for {:x?}", dev.uuid),
                                                                Err(e) => tracing::error!("Failed to save zero {:x?}: {}", dev.uuid, e),
                                                            }
                                                        }
                                                    }
                                                }, "Save Zero" }
                                            }
                                            li {
                                                class: "flex items-center gap-2",

                                                label {
                                                    r#for: "shot-delay-input-{slot}",
                                                    class: "text-sm text-gray-900 dark:text-white",
                                                    "Shot Delay (ms):"
                                                }

                                                div {
                                                    class: "relative flex items-center max-w-[6rem]",

                                                    button {
                                                        id: "decrement-button",
                                                        class: "bg-gray-100 dark:bg-gray-700 dark:hover:bg-gray-600 dark:border-gray-600 \
                                                                hover:bg-gray-200 border border-gray-300 rounded-s-lg p-2 h-10 focus:ring-gray-100 \
                                                                dark:focus:ring-gray-700 focus:ring-2 focus:outline-none",
                                                        onclick: move |_| {
                                                            let mut val = device_signals.peek()[slot].shot_delay.peek().clone();
                                                            if val > 0 {
                                                                val -= 1;
                                                                device_signals.write()[slot].shot_delay.set(val as u16);
                                                            }
                                                        },
                                                        svg {
                                                            class: "w-3 h-3 text-gray-900 dark:text-white",
                                                            xmlns: "http://www.w3.org/2000/svg",
                                                            fill: "none",
                                                            view_box: "0 0 18 2",
                                                            path {
                                                                d: "M1 1h16",
                                                                stroke: "currentColor",
                                                                stroke_linecap: "round",
                                                                stroke_linejoin: "round",
                                                                stroke_width: "2",
                                                            }
                                                        }
                                                    }

                                                    input {
                                                        r#type: "text",
                                                        inputmode: "numeric",
                                                        pattern: "[0-9]*",
                                                        id: "shot-delay-input-{slot}",
                                                        value: "{device_signals.read()[slot].shot_delay}",
                                                        class: "bg-gray-50 border-x-0 border-gray-300 h-10 text-center text-gray-900 text-sm \
                                                                focus:ring-blue-500 focus:border-blue-500 block w-full py-1.5 \
                                                                dark:bg-gray-700 dark:border-gray-600 dark:placeholder-gray-400 \
                                                                dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500",
                                                        oninput: move |e| {
                                                            if let Ok(new_val) = e.value().parse::<i32>() {
                                                                if new_val < 0 {
                                                                    device_signals.write()[slot].shot_delay.set(0);
                                                                } else if new_val <= 1023 {
                                                                    device_signals.write()[slot].shot_delay.set(new_val as u16);
                                                                } else {
                                                                    device_signals.write()[slot].shot_delay.set(1023);
                                                                }
                                                            }
                                                        }
                                                    }

                                                    button {
                                                        id: "increment-button",
                                                        class: "bg-gray-100 dark:bg-gray-700 dark:hover:bg-gray-600 dark:border-gray-600 \
                                                                hover:bg-gray-200 border border-gray-300 rounded-e-lg p-2 h-10 focus:ring-gray-100 \
                                                                dark:focus:ring-gray-700 focus:ring-2 focus:outline-none",
                                                        onclick: move |_| {
                                                            let mut val = device_signals.peek()[slot].shot_delay.peek().clone();
                                                            if val < 1023 {
                                                                val += 1;
                                                                device_signals.write()[slot].shot_delay.set(val as u16);
                                                            }
                                                        },
                                                        svg {
                                                            class: "w-3 h-3 text-gray-900 dark:text-white",
                                                            xmlns: "http://www.w3.org/2000/svg",
                                                            fill: "none",
                                                            view_box: "0 0 18 18",
                                                            path {
                                                                d: "M9 1v16M1 9h16",
                                                                stroke: "currentColor",
                                                                stroke_linecap: "round",
                                                                stroke_linejoin: "round",
                                                                stroke_width: "2",
                                                            }
                                                        }
                                                    }
                                                }

                                                // Set button
                                                button {
                                                    class: "btn-secondary",
                                                    onclick: {
                                                        let dev = device.clone();
                                                        move |_| {
                                                            let dev = dev.clone();
                                                            let delay = *device_signals.peek()[slot].shot_delay.peek();
                                                            async move {
                                                                match (hub().client)().set_shot_delay(dev.clone(), delay).await {
                                                                    Ok(_) => tracing::info!("Set shot delay {}ms for {:x?}", delay, dev.uuid),
                                                                    Err(e) => tracing::error!("Failed to set shot delay {:x?}: {}", dev.uuid, e),
                                                                }
                                                            }
                                                        }
                                                    },
                                                    "Set"
                                                }

                                                // Reset button
                                                button {
                                                    class: "btn-secondary",
                                                    onclick: {
                                                        let dev = device.clone();
                                                        move |_| {
                                                            let dev = dev.clone();
                                                            async move {
                                                                match (hub().client)().reset_shot_delay(dev.clone()).await {
                                                                    Ok(delay_ms) => {
                                                                        tracing::info!("Reset shot delay for {:x?}", dev.uuid);
                                                                        device_signals.write()[slot].shot_delay.set(delay_ms);
                                                                    }
                                                                    Err(e) => tracing::error!("Failed to reset shot delay {:x?}: {}", dev.uuid, e),
                                                                }
                                                            }
                                                        }
                                                    },
                                                    "Reset"
                                                }

                                                // Save button (sets before saving)
                                                button {
                                                    class: "btn-secondary",
                                                    onclick: {
                                                        let dev = device.clone();
                                                        move |_| {
                                                            let dev = dev.clone();
                                                            let delay = *device_signals.peek()[slot].shot_delay.peek();
                                                            async move {
                                                                match (hub().client)().set_shot_delay(dev.clone(), delay).await {
                                                                    Ok(_) => {
                                                                        tracing::info!("Set shot delay {}ms for {:x?}", delay, dev.uuid);
                                                                        match (hub().client)().save_shot_delay(dev.clone()).await {
                                                                            Ok(_) => tracing::info!("Saved shot delay for {:x?}", dev.uuid),
                                                                            Err(e) => tracing::error!("Failed to save shot delay {:x?}: {}", dev.uuid, e),
                                                                        }
                                                                    }
                                                                    Err(e) => tracing::error!("Failed to set shot delay {:x?}: {}", dev.uuid, e),
                                                                }
                                                            }
                                                        }
                                                    },
                                                    "Save"
                                                }
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
                CrosshairManager { hub, tracking_sender: Some(tracking_sender) },
            }

            button {
                class: "fixed z-50 top-4 right-4 …",
                onclick: move |_| dioxus::desktop::window().close(),
                "Close"
            }
        }
    }
}
