use std::collections::HashMap;

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
    // 1) players + seen
    // debug
    // let mut players = use_signal(|| vec![Player { id: [0; 6], pos: (0.0, 0.0) }]);
    let mut players = use_signal(HashMap::<odyssey_hub_common::device::Device, Player>::new);

    let mut root_div = use_signal(|| None);

    // 2) signal to hold the container's Rect
    let container_rect = use_signal(Rect::zero);

    // 3) onmounted on root <div> to measure once
    let mut rect_signal = container_rect.clone();

    // 4) event loop: read new events, map normalized aimpoint → pixel coords
    use_effect(move || {
        let rect = container_rect.read();
        let width = rect.width() as f64;
        let height = rect.height() as f64;
        let origin_x = rect.origin.x as f64;
        let origin_y = rect.origin.y as f64;

        match (hub().latest_event)() {
            oe::Event::DeviceEvent(oe::DeviceEvent {
                device,
                kind: oe::DeviceEventKind::TrackingEvent(oe::TrackingEvent { aimpoint, .. }),
            }) =>
            {
                let x = origin_x + (aimpoint.x as f64) * width;
                let y = origin_y + (aimpoint.y as f64) * height;

                if players.peek().contains_key(&device) {
                    players.write().get_mut(&device).unwrap().pos = (x, y);
                } else {
                    players.write().insert(
                        device,
                        Player {
                            pos: (x, y),
                        }
                    );
                }
            }
            oe::Event::DeviceEvent(oe::DeviceEvent {
                device,
                kind: oe::DeviceEventKind::DisconnectEvent
            }) => {
                players.write().remove(&device);
            }
            _ => {}
        }
    });

    let hub_peek = hub.peek();
    let devices_peek = hub_peek.devices.peek();
    let children = devices_peek.iter().filter_map(|(key, device)| {
        let players = players();
        let player = players.get(&device);
        if let Some(player) = player {
            Some(rsx! {
                img {
                    key: "{key}",
                    class: "absolute pointer-events-none translate-x-[-50%] translate-y-[-50%]",
                    transform_origin: "center",
                    transform: format!("translate({:.0}px, {:.0}px)", player.pos.0, player.pos.1),
                    src: CROSSHAIRS[key.min(CROSSHAIRS.len() - 1)],
                    alt: "crosshair"
                }
            })
        } else {
            None
        }
    });

    rsx! {
        div {
            class: "w-full h-full",
            onmounted: move |cx| async move {
                let cx_data = cx.data();
                let client_rect = cx_data.as_ref().get_client_rect();
                if let Ok(rect) = client_rect.await {
                    rect_signal.set(rect);
                }
                root_div.set(Some(cx_data));
            },
            onresize: move |_| async move {
                if let Some(root_div) = root_div() {
                    let client_rect = root_div.as_ref().get_client_rect();
                    if let Ok(rect) = client_rect.await {
                        rect_signal.set(rect);
                    }
                }
            },
            div {
                class: "absolute inset-0 overflow-hidden",
                // debug
                // onmousemove: move |evt: MouseEvent| {
                //     let coords = evt.data;
                //     let mut list = players.write();
                //     if let Some(p) = list.get_mut(0) {
                //         let ec = coords.element_coordinates();
                //         p.pos = (ec.x, ec.y);
                //     }
                // },
                // end debug
                {children}
            }
        }
    }
}
