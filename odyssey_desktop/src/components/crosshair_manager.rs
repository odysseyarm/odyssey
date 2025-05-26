use dioxus::prelude::*;
use dioxus::desktop::{use_wry_event_handler, tao};

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
pub fn CrosshairManager() -> Element {
    let mut players = use_signal(|| vec![Player { id: [0; 6], pos: (0.0, 0.0) }]);

    let snapshot = players();

    let children = snapshot
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
            onmousemove: move |evt: MouseEvent| {
                let coords = evt.data;
                let mut list = players.write();
                if let Some(p) = list.get_mut(0) {
                    let ec = coords.element_coordinates();
                    p.pos = (ec.x, ec.y);
                }
            },
            // end debug
            {children}
        }
    }
}
