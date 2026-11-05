//! The Pairing page: every dongle and device that can be paired.

mod device_row;
mod dongle_row;
mod pair_buttons;
mod pairing_state;

use std::collections::HashMap;

use device_row::DeviceRow;
use dongle_row::DongleRow;
use gpui::{
    AppContext as _, Context, Entity, IntoElement as _, ParentElement as _, Render, Styled as _,
    Window, div,
};

use crate::components::ui::{card, card_title, page};
use crate::hub::{Hub, HubHandle};
use crate::theme::palette;

/// The font of ids and addresses.
const MONO_FONT: &str = if cfg!(windows) {
    "Consolas"
} else {
    "monospace"
};

pub(crate) struct PairingView {
    hub: Entity<Hub>,
    // A row keeps its status and pending command for as long as its dongle or
    // device is listed, so rows are keyed rather than rebuilt.
    dongle_rows: HashMap<String, Entity<DongleRow>>,
    device_rows: HashMap<[u8; 6], Entity<DeviceRow>>,
}

impl PairingView {
    pub(crate) fn new(_window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let hub = cx.global::<HubHandle>().0.clone();
        cx.observe(&hub, |_, _, cx| cx.notify()).detach();
        Self {
            hub,
            dongle_rows: HashMap::new(),
            device_rows: HashMap::new(),
        }
    }
}

impl Render for PairingView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl gpui::IntoElement {
        let p = palette(cx);
        let hub = self.hub.clone();
        let (dongles, devices) = {
            let hub = hub.read(cx);
            (hub.dongles.clone(), hub.devices.clone())
        };

        self.dongle_rows
            .retain(|id, _| dongles.iter().any(|dongle| dongle.id == *id));
        self.device_rows
            .retain(|uuid, _| devices.iter().any(|device| device.uuid == *uuid));

        let mut sections = Vec::new();

        if dongles.is_empty() && devices.is_empty() {
            sections.push(
                card(p)
                    .child(card_title(p, "Nothing to pair yet").mb_1())
                    .child(div().text_sm().text_color(p.fg_muted).child(
                        "Plug a dongle or a device into this computer. They appear here as soon as the hub finds them.",
                    ))
                    .into_any_element(),
            );
        }

        if !dongles.is_empty() {
            let rows = dongles.iter().map(|dongle| {
                self.dongle_rows
                    .entry(dongle.id.clone())
                    .or_insert_with(|| {
                        cx.new(|cx| DongleRow::new(hub.clone(), dongle.id.clone(), cx))
                    })
                    .clone()
            });
            sections.push(
                card(p)
                    .child(card_title(p, "Dongles").mb_3())
                    .child(div().flex().flex_col().gap_2().children(rows))
                    .into_any_element(),
            );
        }

        if !devices.is_empty() {
            let rows = devices.iter().map(|device| {
                let row = self
                    .device_rows
                    .entry(device.uuid)
                    .or_insert_with(|| cx.new(|cx| DeviceRow::new(hub.clone(), device.clone(), cx)))
                    .clone();
                row.update(cx, |row, cx| row.set_device(device.clone(), cx));
                row
            });
            sections.push(
                card(p)
                    .child(card_title(p, "Devices").mb_3())
                    .child(div().flex().flex_col().gap_2().children(rows))
                    .into_any_element(),
            );
        }

        page(
            p,
            "Pairing",
            Some("Pair a device with a dongle to use it without a cable. A dongle remembers the devices it has bonded with.".into()),
            sections,
        )
    }
}
