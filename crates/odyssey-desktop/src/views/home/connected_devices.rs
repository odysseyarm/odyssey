//! The list of devices the hub reports, plus any being updated that it does not.

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div,
};

use super::{HomeEvent, HomeView};
use crate::components::firmware_update::{dongle_update_card, updating_device_row};
use crate::components::ui;
use crate::theme::Palette;

impl HomeView {
    /// One card per dongle running firmware older than the manifest's newest.
    pub(super) fn dongle_updates(&self, p: Palette, cx: &Context<'_, Self>) -> Vec<AnyElement> {
        let manifest = self.manifest.read(cx).manifest.clone();
        self.hub
            .read(cx)
            .dongles
            .iter()
            .filter_map(|dongle| dongle_update_card(dongle, manifest.as_ref(), p))
            .collect()
    }

    pub(super) fn connected_devices(&self, p: Palette, cx: &mut Context<'_, Self>) -> AnyElement {
        let devices = self.hub.read(cx).devices.clone();
        let updating = self.manager.get_visible_updates();
        if devices.is_empty() && updating.is_empty() {
            return ui::card(p)
                .gap_1()
                .child(ui::card_title(p, "No devices connected"))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .text_sm()
                        .text_color(p.fg_muted)
                        .child("Plug a device in with its USB cable, or")
                        .child(
                            div()
                                .flex()
                                .child(
                                    div()
                                        .id("pair-link")
                                        .cursor_pointer()
                                        .underline()
                                        .text_color(p.accent_ink)
                                        .on_click(cx.listener(|_, _, _, cx| {
                                            cx.emit(HomeEvent::OpenPairing);
                                        }))
                                        .child("pair it with a dongle"),
                                )
                                .child("."),
                        ),
                )
                .into_any_element();
        }

        let mut list = div().flex().flex_col().gap_2();
        for device in &devices {
            if let Some(row) = self.rows.get(&device.uuid) {
                list = list.child(row.clone());
            }
        }
        for uuid in updating
            .iter()
            .filter(|uuid| !devices.iter().any(|device| &device.uuid == *uuid))
        {
            list = list.child(updating_device_row(*uuid, &self.manager, p));
        }
        ui::card(p)
            .child(div().mb_3().child(ui::card_title(p, "Device details")))
            .child(list)
            .into_any_element()
    }
}
