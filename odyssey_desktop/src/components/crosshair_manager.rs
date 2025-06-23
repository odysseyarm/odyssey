// src/components/crosshair_manager.rs

use dioxus::{html::geometry::euclid::Rect, prelude::*};
use odyssey_hub_common::events as oe;

#[derive(Clone)]
pub struct Player {
    pub pos: (f64, f64),
}

const CROSSHAIRS: [Asset; 6] = [
    asset!("/assets/images/crosshairs/crosshair-red.png"),
    asset!("/assets/images/crosshairs/crosshair-blue.png"),
    asset!("/assets/images/crosshairs/crosshair-yellow.png"),
    asset!("/assets/images/crosshairs/crosshair-green.png"),
    asset!("/assets/images/crosshairs/crosshair-orange.png"),
    asset!("/assets/images/crosshairs/crosshair-gray.png"),
];

#[component]
pub fn CrosshairManager(hub: Signal<crate::hub::HubContext>) -> Element {
    let mut players = use_signal(|| Vec::<Option<Player>>::new());

    let mut root_div = use_signal(|| None);
    let mut rect_signal = use_signal(Rect::zero);

    let window = dioxus::desktop::use_window();
    let window_size = window.inner_size().to_logical::<f64>(window.scale_factor());

    // effect to update positions / clear on disconnect
    use_effect(move || {
        if let Some(oe::Event::DeviceEvent(oe::DeviceEvent(device, kind))) = (hub().latest_event)()
        {
            let rect = rect_signal.read();
            let origin_x = rect.origin.x as f64;
            let origin_y = rect.origin.y as f64;

            match kind {
                oe::DeviceEventKind::TrackingEvent(oe::TrackingEvent { aimpoint, .. }) => {
                    let x = aimpoint.x as f64 * window_size.width - origin_x;
                    let y = aimpoint.y as f64 * window_size.height - origin_y;

                    if let Some(key) = hub.peek().device_key(&device) {
                        let mut w = players.write();
                        if key >= w.len() {
                            w.resize_with(key + 1, || None);
                        }
                        w[key] = Some(Player { pos: (x, y) });
                    }
                }
                oe::DeviceEventKind::DisconnectEvent => {
                    if let Some(key) = hub.peek().device_key(&device) {
                        let mut w = players.write();
                        if key < w.len() {
                            w[key] = None;
                        }
                    }
                }
                _ => {}
            }
        }
    });

    let devices = (hub().devices)();
    {
        let mut w = players.write();
        if w.len() < devices.len() {
            w.resize_with(devices.len(), || None);
        }
    }

    let children =
        devices.iter().filter_map(|(key, _device)| {
            players()[key].as_ref().map(|player| rsx! {
            img {
                key: "{key}",
                class: "absolute pointer-events-none translate-x-[-50%] translate-y-[-50%]",
                transform_origin: "center",
                transform: format!("translate({:.0}px, {:.0}px)", player.pos.0, player.pos.1),
                src: CROSSHAIRS[key.min(CROSSHAIRS.len() - 1)],
                alt: "crosshair"
            }
        })
        });

    rsx! {
        div {
            class: "w-full h-full",
            onmounted: move |cx| async move {
                let cx_data = cx.data();
                if let Ok(rect) = cx_data.as_ref().get_client_rect().await {
                    rect_signal.set(rect);
                }
                root_div.set(Some(cx_data));
            },
            onresize: move |_| async move {
                if let Some(root) = root_div() {
                    if let Ok(rect) = root.as_ref().get_client_rect().await {
                        rect_signal.set(rect);
                    }
                }
            },
            div {
                class: "absolute inset-0 overflow-hidden",
                {children}
            }
        }
    }
}
