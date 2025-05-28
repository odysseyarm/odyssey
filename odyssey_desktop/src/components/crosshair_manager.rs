use std::collections::HashSet;

use dioxus::prelude::*;

use odyssey_hub_common::events as oe;

pub type PlayerId = [u8; 6];

#[derive(Clone)]
pub struct Player {
    pub id: PlayerId,
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
    // debug
    // let mut players = use_signal(|| vec![Player { id: [0; 6], pos: (0.0, 0.0) }]);
    let mut players = use_signal(Vec::new);

    let mut seen = use_signal(HashSet::new);

    use_effect(move || {
        for event in hub().events.read().iter() {
            if let oe::Event::DeviceEvent(oe::DeviceEvent {
                device,
                kind: oe::DeviceEventKind::TrackingEvent(oe::TrackingEvent { aimpoint, .. }),
            }) = event
            {
                let id = device.uuid();
                if seen.write().insert(id) {
                    players.push(Player {
                        id,
                        pos: (aimpoint.x as f64, aimpoint.y as f64),
                    });
                } else {
                    if let Some(mut player) = players.iter_mut().find(|p| p.id == id) {
                        // todo figure out how to transform aimpoint
                        player.pos = (aimpoint.x as f64 * 200., aimpoint.y as f64 * 200.);
                    }
                }
            }
        }
    });

    let children = players
        .iter()
        .enumerate()
        .map(|(i, player)| {
            let key = player.id.iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>();
            rsx! {
                img {
                    key: "{key}",
                    class: "absolute pointer-events-none translate-x-[-50%] translate-y-[-50%]",
                    transform_origin: "center",
                    transform: format!("translate({:.0}px, {:.0}px)", player.pos.0, player.pos.1),
                    src: CROSSHAIRS[i.min(CROSSHAIRS.len() - 1)],
                    alt: "crosshair"
                }
            }
        });

    rsx! {
        div {
            class: "absolute inset-0",
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
