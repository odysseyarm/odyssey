//! A dongle, its pairing controls, and the devices it has bonded or sees.

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window,
    div, px,
};

use super::MONO_FONT;
use super::pair_buttons::PairingControls;
use super::pairing_state::PairTarget;
use crate::hub::Hub;
use crate::theme::{RADIUS, palette};
use crate::views::device_label;
use crate::views::device_label::format_address;

/// The dongle control protocol that can list its bonds.
const BONDS_PROTOCOL_VERSION: [u16; 3] = [0, 1, 1];
/// The firmware version shown for a dongle until it reports its own.
const UNKNOWN_FIRMWARE_VERSION: [u16; 3] = [0, 0, 0];

pub(super) struct DongleRow {
    id: String,
    hub: Entity<Hub>,
    controls: Entity<PairingControls>,
}

impl DongleRow {
    pub(super) fn new(hub: Entity<Hub>, id: String, cx: &mut Context<'_, Self>) -> Self {
        let target = PairTarget::Dongle(id.clone());
        let controls = cx.new(|cx| PairingControls::new(hub.clone(), target, cx));
        cx.observe(&controls, |_, _, cx| cx.notify()).detach();
        // Device labels in the bond list follow the hub's device list.
        cx.observe(&hub, |_, _, cx| cx.notify()).detach();
        Self { id, hub, controls }
    }
}

impl Render for DongleRow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let p = palette(cx);
        let hub = self.hub.read(cx);
        let Some(dongle) = hub.dongles.iter().find(|dongle| dongle.id == self.id) else {
            return div();
        };
        let status = self.controls.read(cx).status().to_string();

        let firmware = &dongle.firmware_version;
        let firmware_suffix = if *firmware == UNKNOWN_FIRMWARE_VERSION {
            String::new()
        } else {
            format!(" (fw {}.{}.{})", firmware[0], firmware[1], firmware[2])
        };

        let mut heading = div()
            .flex()
            .flex_wrap()
            .min_w_0()
            .items_center()
            .gap_x_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(p.fg)
                    .child("Dongle"),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(MONO_FONT)
                    .text_color(p.fg_muted)
                    .child(format!("{}{firmware_suffix}", dongle.id)),
            );
        if !status.is_empty() {
            heading = heading.child(
                div()
                    .text_xs()
                    .italic()
                    .text_color(p.fg_muted)
                    .child(status),
            );
        }

        let mut row = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded(px(RADIUS))
            .bg(p.surface_alt)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(heading)
                    .child(self.controls.clone()),
            );

        // Dongles that can list their bonds show those; older ones only show who
        // is connected right now.
        let lists_bonds = dongle.control_protocol_version >= BONDS_PROTOCOL_VERSION;
        let (listed_addresses, list_label) = if lists_bonds {
            (&dongle.bonded_devices, "Bonds")
        } else {
            (&dongle.connected_devices, "Connected")
        };
        if !listed_addresses.is_empty() {
            let mut list = div().ml_4().flex().flex_col().gap_1().child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(p.fg_muted)
                    .child(list_label.to_uppercase()),
            );
            for address in listed_addresses {
                let connected = dongle.connected_devices.contains(address);
                let label = hub
                    .devices
                    .iter()
                    .find(|device| device.uuid == *address)
                    .map_or_else(|| format_address(address), device_label);
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            // A bonded device that is switched off is normal, not an error, so its dot is neutral.
                            div().size_2().rounded_full().bg(if connected {
                                p.success
                            } else {
                                p.fg_faint
                            }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family(MONO_FONT)
                                .text_color(p.fg_muted)
                                .child(label),
                        )
                        .child(div().text_xs().text_color(p.fg_muted).child(if connected {
                            "connected"
                        } else {
                            "not in range"
                        })),
                );
            }
            row = row.child(list);
        }
        row
    }
}
