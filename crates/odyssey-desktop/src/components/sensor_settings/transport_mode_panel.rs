use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::{Disableable as _, Sizable as _};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport};

use super::op_status::{OpStatus, rpc_failure, status_line};
use super::product_ids::supports_transport_switching;
use crate::hub::HubHandle;
use crate::runtime;
use crate::theme::{RADIUS_SM, palette};

/// Shows a transport mode selector (BLE / USB) for ATS Lite / Lite1 devices.
/// Requires only CONTROL capability, so it is visible even when the device is
/// in BLE mode and the EVENTS bulk endpoint is not available. Firmware with
/// `AUTO_TRANSPORT` picks its link itself and gets no selector.
pub(crate) struct TransportModePanel {
    device: Device,
    /// Distinguishes this panel's element ids from other rows'.
    key: String,
    is_open: bool,
    details_open: bool,
    status: OpStatus,
    /// The mode the user asked for, `None` when there is no override. Cleared
    /// once the device shows that mode, which happens after the reboot.
    user_pending_usb: Option<bool>,
    switching: bool,
}

impl TransportModePanel {
    pub(crate) fn new(device: Device, _cx: &mut Context<Self>) -> Self {
        Self {
            key: hex::encode(device.uuid),
            device,
            is_open: false,
            details_open: false,
            status: OpStatus::Idle,
            user_pending_usb: None,
            switching: false,
        }
    }

    pub(crate) fn set_device(&mut self, device: Device, cx: &mut Context<Self>) {
        if device == self.device {
            return;
        }
        self.device = device;
        let device_is_usb = self.device_is_usb();
        if self.user_pending_usb == Some(device_is_usb) {
            self.user_pending_usb = None;
            let mode = if device_is_usb { "USB" } else { "Bluetooth" };
            self.status = OpStatus::Done(format!("Connected using {mode}."));
        }
        cx.notify();
    }

    fn device_is_usb(&self) -> bool {
        self.device.events_transport == EventsTransport::Wired
    }

    /// Asks the hub to switch the device's events transport and reports
    /// progress through `status`. The device reboots, so success only means
    /// "requested".
    fn switch_mode(&mut self, usb_mode: bool, cx: &mut Context<Self>) {
        if self.switching {
            return;
        }
        self.switching = true;
        self.status = OpStatus::Busy("Switching...".into());
        cx.notify();
        let Some(client) = cx.global::<HubHandle>().0.read(cx).client() else {
            tracing::warn!("set_transport_mode failed: not connected to the hub");
            self.user_pending_usb = None;
            self.status = rpc_failure("switch the transport mode", &"not connected to the hub");
            self.switching = false;
            return;
        };
        let device = self.device.clone();
        cx.spawn(async move |this, cx| {
            let result =
                runtime::spawn(
                    async move { client.device(device).set_transport_mode(usb_mode).await },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(confirmed_usb)) => {
                        let mode = if confirmed_usb { "USB" } else { "Bluetooth" };
                        if this.device_is_usb() == confirmed_usb {
                            this.user_pending_usb = None;
                            this.status = OpStatus::Done(format!("Connected using {mode}."));
                        } else {
                            this.user_pending_usb = Some(confirmed_usb);
                            this.status = OpStatus::Busy(format!(
                                "Switching to {mode}. The device restarts and reconnects on its own."
                            ));
                        }
                    }
                    Ok(Err(error)) => {
                        tracing::error!("set_transport_mode failed: {error:#}");
                        this.user_pending_usb = None;
                        this.status = rpc_failure("switch the transport mode", &error);
                    }
                    Err(error) => {
                        tracing::error!("set_transport_mode task failed: {error}");
                        this.user_pending_usb = None;
                        this.status = rpc_failure("switch the transport mode", &error);
                    }
                }
                this.switching = false;
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for TransportModePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let capabilities = self.device.capabilities;
        let should_render = supports_transport_switching(self.device.product_id)
            && capabilities.contains(DeviceCapabilities::CONTROL)
            && !capabilities.contains(DeviceCapabilities::AUTO_TRANSPORT);
        if !should_render {
            return div();
        }
        let palette = &palette(cx);
        // The displayed mode prefers the user's request while the reboot is
        // pending, and otherwise follows the device.
        let pending_usb = self
            .user_pending_usb
            .unwrap_or_else(|| self.device_is_usb());
        let key = &self.key;
        let mode_button = |name: &str, label: &'static str, usb_mode: bool| {
            let active = pending_usb == usb_mode;
            let button = Button::new(SharedString::from(format!("transport-{name}-{key}")))
                .label(label)
                .small()
                .disabled(self.switching || active)
                .on_click(cx.listener(move |this, _, _, cx| this.switch_mode(usb_mode, cx)));
            if active {
                button.primary()
            } else {
                button.outline()
            }
        };

        div()
            .mt_2()
            .child(
                div()
                    .id(SharedString::from(format!("transport-toggle-{key}")))
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(palette.fg)
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.accent_ink))
                    .child(if self.is_open {
                        "▾ Connection"
                    } else {
                        "▸ Connection"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.is_open = !this.is_open;
                        cx.notify();
                    })),
            )
            .when(self.is_open, |this| {
                this.child(
                div()
                    .mt_2()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .bg(palette.surface_alt)
                    .rounded(px(RADIUS_SM))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_sm()
                                    .whitespace_nowrap()
                                    .text_color(palette.fg_muted)
                                    .child("Transport mode"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_3()
                                    .child(mode_button("bluetooth", "Bluetooth", false))
                                    .child(mode_button("usb", "USB", true)),
                            ),
                    )
                    .child(div().text_xs().text_color(palette.fg_muted).child(if pending_usb {
                        "USB carries sensor data and tuning over the wired connection."
                    } else {
                        "Bluetooth carries sensor data and tuning through a paired Odyssey dongle."
                    }))
            )
            })
            .child(status_line(
                &self.status,
                format!("transport-details-{key}"),
                self.details_open,
                cx.listener(|this, _, _, cx| {
                    this.details_open = !this.details_open;
                    cx.notify();
                }),
                palette,
            ))
    }
}
