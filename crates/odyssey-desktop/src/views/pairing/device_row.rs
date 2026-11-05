//! A device that pairs with a controller, and its pairing controls.

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window,
    div, px,
};
use odyssey_hub_common::device::Device;

use super::MONO_FONT;
use super::pair_buttons::PairingControls;
use super::pairing_state::PairTarget;
use crate::hub::Hub;
use crate::theme::{RADIUS, palette};
use crate::views::device_label;

pub(super) struct DeviceRow {
    device: Device,
    controls: Entity<PairingControls>,
}

impl DeviceRow {
    pub(super) fn new(hub: Entity<Hub>, device: Device, cx: &mut Context<'_, Self>) -> Self {
        let target = PairTarget::Device(device.clone());
        let controls = cx.new(|cx| PairingControls::new(hub, target, cx));
        cx.observe(&controls, |_, _, cx| cx.notify()).detach();
        Self { device, controls }
    }

    /// Follows the hub's latest view of the device, so commands address it as
    /// it is now.
    pub(super) fn set_device(&mut self, device: Device, cx: &mut Context<'_, Self>) {
        if self.device == device {
            return;
        }
        let target = PairTarget::Device(device.clone());
        self.device = device;
        self.controls
            .update(cx, |controls, _| controls.retarget(target));
        cx.notify();
    }
}

impl Render for DeviceRow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let p = palette(cx);
        let status = self.controls.read(cx).status().to_string();
        let mut label = div().flex().items_center().gap_2().child(
            div()
                .text_sm()
                .font_family(MONO_FONT)
                .text_color(p.fg)
                .child(device_label(&self.device)),
        );
        if !status.is_empty() {
            label = label.child(
                div()
                    .text_xs()
                    .italic()
                    .text_color(p.fg_muted)
                    .child(status),
            );
        }
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .p_3()
            .rounded(px(RADIUS))
            .bg(p.surface_alt)
            .child(label)
            .child(self.controls.clone())
    }
}
