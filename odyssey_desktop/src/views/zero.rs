use crate::components::crosshair_manager::{CrosshairManager, TrackingSender};
use crate::hub;
use dioxus::{html::geometry::euclid::Rect, logger::tracing, prelude::*};
use odyssey_hub_client::tracking_history::TrackingHistory;
use odyssey_hub_common::events as oe;
use std::collections::HashMap;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

/// Tracking history ring buffer capacity per device (~2 seconds at 100Hz)
const TRACKING_HISTORY_CAP: usize = 200;

/// Save zero result: None = idle, Some(Ok(())) = saved, Some(Err(msg)) = error
type SaveStatus = Option<Result<(), String>>;

#[derive(Default)]
struct DeviceSignals {
    shooting: Signal<bool>,
    zeroing_on_key: Signal<bool>,
    shot_delay: Signal<u16>,
    save_zero_status: Signal<SaveStatus>,
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

    // Per-device ring buffer of recent tracking events for shot-delay-compensated zeroing
    let tracking_history: Signal<HashMap<[u8; 6], TrackingHistory>> =
        use_signal(|| HashMap::new());

    // Subscribe to tracking events and buffer them per-device
    {
        let mut tracking_history = tracking_history.clone();
        use_future(move || {
            let mut receiver = hub.peek().subscribe_tracking();
            async move {
                loop {
                    match receiver.recv().await {
                        Ok((device, te)) => {
                            tracking_history
                                .write()
                                .entry(device.uuid)
                                .or_insert_with(|| TrackingHistory::new(TRACKING_HISTORY_CAP))
                                .push(te);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });
    }

    // fire off zero() when we see an ImpactEvent and the flag is set.
    // Uses a broadcast channel (like tracking events) so it works across VirtualDom windows.
    {
        use_future(move || {
            let mut receiver = hub.peek().subscribe_impact();
            async move {
                loop {
                    match receiver.recv().await {
                        Ok((device, impact)) => {
                            let hub_ctx = hub.peek().clone();
                            if let Some(key) = hub_ctx.device_key(&device) {
                                let shooting = *device_signals.peek()[key].shooting.peek();
                                tracing::info!(
                                    "ImpactEvent for {:02x?}: slot={}, shooting={}",
                                    device.uuid, key, shooting
                                );
                                if shooting {
                                    let zr = *zero_screen_ratio.peek();
                                    let shot_delay_ms =
                                        *device_signals.peek()[key].shot_delay.peek();
                                    // Convert ms → µs (device timestamp unit). For accessories
                                    // (u32::MAX sentinel), get the latest entry's timestamp and
                                    // subtract from that, giving ~0–10ms staleness at 100Hz.
                                    let shot_delay_us = shot_delay_ms as u32 * 1000;
                                    let history = tracking_history.peek();
                                    let lookup_ts = if impact.timestamp == u32::MAX {
                                        history
                                            .get(&device.uuid)
                                            .and_then(|h| h.latest())
                                            .map(|te| te.timestamp.wrapping_sub(shot_delay_us))
                                            .unwrap_or(u32::MAX)
                                    } else {
                                        impact.timestamp.wrapping_sub(shot_delay_us)
                                    };
                                    drop(history);
                                    let historical_te = tracking_history
                                        .peek()
                                        .get(&device.uuid)
                                        .and_then(|h| h.get_closest(lookup_ts));
                                    device_signals.write()[key].shooting.set(false);
                                    let dev = device.clone();
                                    if let Err(e) = hub_ctx
                                        .client
                                        .peek()
                                        .clone()
                                        .zero(
                                            dev.clone(),
                                            nalgebra::Vector3::new(0., -0.0635, 0.).into(),
                                            nalgebra::Vector2::new(zr.0, zr.1).into(),
                                            historical_te,
                                        )
                                        .await
                                    {
                                        tracing::error!(
                                            "Failed to zero device {:x?}: {}",
                                            dev.uuid,
                                            e
                                        );
                                    }
                                }
                            } else {
                                tracing::warn!(
                                    "ImpactEvent: device_key not found for {:02x?}",
                                    device.uuid
                                );
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });
    }

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
            tabindex: "0",
            autofocus: true,
            onkeydown: move |e: KeyboardEvent| async move {
                let key_str = e.key().to_string();
                if key_str != "z" && key_str != "Escape" { return; }

                let hub_ctx = hub.peek().clone();
                let devs: Vec<_> = (hub_ctx.devices)().into_iter().collect();
                let slots_len = device_signals.peek().len();

                for slot in 0..slots_len {
                    if key_str == "z" && *device_signals.peek()[slot].zeroing_on_key.peek() {
                        device_signals.write()[slot].zeroing_on_key.set(false);
                        if let Some((_, dev)) = devs.iter().find(|(s, _)| *s == slot) {
                            let dev = dev.clone();
                            let zr = *zero_screen_ratio.peek();
                            let shot_delay_ms = *device_signals.peek()[slot].shot_delay.peek();
                            let shot_delay_us = shot_delay_ms as u32 * 1000;
                            let historical_te = tracking_history
                                .peek()
                                .get(&dev.uuid)
                                .and_then(|h| {
                                    let lookup_ts = h
                                        .latest()
                                        .map(|te| te.timestamp.wrapping_sub(shot_delay_us))
                                        .unwrap_or(u32::MAX);
                                    h.get_closest(lookup_ts)
                                });
                            if let Err(e) = hub_ctx.client.peek().clone()
                                .zero(
                                    dev.clone(),
                                    nalgebra::Vector3::new(0., -0.0635, 0.).into(),
                                    nalgebra::Vector2::new(zr.0, zr.1).into(),
                                    historical_te,
                                )
                                .await
                            {
                                tracing::error!("Failed to zero device {:x?}: {}", dev.uuid, e);
                            }
                        }
                    }
                    if key_str == "Escape" && *device_signals.peek()[slot].zeroing_on_key.peek() {
                        device_signals.write()[slot].zeroing_on_key.set(false);
                    }
                }
            },

            aside {
                class: "fixed top-0 left-0 z-40 h-screen transition-transform -translate-x-full sm:translate-x-0",
                div {
                    class: "h-full px-3 py-4 overflow-y-auto bg-gray-100/5 dark:bg-gray-900/5",
                    ul { class: "space-y-2 font-medium",
                        for (slot, device) in devices {
                            {
                                let firing = (device_signals.read()[slot].shooting)();
                                let zeroing_on_key = (device_signals.read()[slot].zeroing_on_key)();
                                let label = crate::views::device_label(&device);
                                rsx! {
                                    li {
                                        class: "flex flex-col gap-2",
                                        span {
                                            class: "text-gray-900 dark:text-white",
                                            {label},
                                        }
                                        ul { class: "flex flex-col gap-2",
                                            li { class: "flex items-center gap-3 flex-wrap",
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
                                                button {
                                                    class: if zeroing_on_key { "btn-secondary-danger" } else { "btn-secondary" },
                                                    onclick: move |_| {
                                                        let current = *device_signals.peek()[slot].zeroing_on_key.peek();
                                                        device_signals.write()[slot].zeroing_on_key.set(!current);
                                                    },
                                                    if zeroing_on_key { "Cancel [Z]" } else { "Zero on [Z]" }
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
                                                {
                                                    let save_status = (device_signals.read()[slot].save_zero_status)();
                                                    let (btn_class, label) = match &save_status {
                                                        None => ("btn-secondary", "Save Zero".to_string()),
                                                        Some(Ok(())) => ("btn-secondary-success", "Saved".to_string()),
                                                        Some(Err(e)) => ("btn-secondary-danger", format!("Error: {e}")),
                                                    };
                                                    rsx! {
                                                        button { class: btn_class, onclick: {
                                                            let dev = device.clone();
                                                            move |_| {
                                                                let dev = dev.clone();
                                                                async move {
                                                                    match (hub().client)().save_zero(dev.clone()).await {
                                                                        Ok(_) => {
                                                                            tracing::info!("Saved zero for {:x?}", dev.uuid);
                                                                            device_signals.write()[slot].save_zero_status.set(Some(Ok(())));
                                                                            // Reset back to idle after 2 seconds
                                                                            spawn(async move {
                                                                                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                                                                device_signals.write()[slot].save_zero_status.set(None);
                                                                            });
                                                                        }
                                                                        Err(e) => {
                                                                            let msg = e.to_string();
                                                                            tracing::error!("Failed to save zero {:x?}: {}", dev.uuid, msg);
                                                                            device_signals.write()[slot].save_zero_status.set(Some(Err(msg)));
                                                                            spawn(async move {
                                                                                tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                                                                                device_signals.write()[slot].save_zero_status.set(None);
                                                                            });
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }, {label} }
                                                    }
                                                }
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
                class: "fixed z-50 top-4 right-4 w-10 h-10 rounded-full bg-gray-900/60 hover:bg-gray-900/80 dark:bg-white/15 dark:hover:bg-white/30 text-white text-xl flex items-center justify-center transition-colors",
                onclick: move |_| dioxus::desktop::window().close(),
                "✕"
            }
        }
    }
}
