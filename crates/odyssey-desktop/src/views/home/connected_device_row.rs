//! A single row in the connected devices list.
//!
//! An entity of its own so the rename field and the settings panels keep their
//! state while the device list around them changes.

use std::time::Duration;

use gpui::component::input::{Input, InputEvent, InputState};
use gpui::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, Context, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, pulsating_between, px,
};
use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};

use super::firmware_state::FirmwareManifestModel;
use crate::components::firmware_update::{
    FirmwareUpdateManager, UpdateTicker, device_firmware_update,
};
use crate::components::image_viewer::ImageViewer;
use crate::components::sensor_settings::{SensorSettings, TransportModePanel};
use crate::components::ui::{ButtonKind, button};
use crate::hub::Hub;
use crate::runtime;
use crate::theme::{self, Palette, RADIUS};

/// The firmware version shown for a device until it reports its own.
const UNKNOWN_FIRMWARE_VERSION: [u16; 3] = [0, 0, 0];

fn transport_name(transport: Transport) -> &'static str {
    match transport {
        Transport::Usb => "USB",
        Transport::UsbMux => "BLE",
        Transport::UdpMux => "UDP",
    }
}

/// What to add after the transport name when the device is reached some other
/// way than the name says.
fn transport_suffix(device: &Device) -> &'static str {
    let has_control = device.capabilities.contains(DeviceCapabilities::CONTROL);
    match (device.transport, device.events_transport) {
        (Transport::Usb, EventsTransport::Bluetooth) => " (BLE events)",
        (Transport::UsbMux, _) if has_control => " (USB control)",
        _ => "",
    }
}

fn is_dfu(device: &Device) -> bool {
    device.capabilities.contains(DeviceCapabilities::DFU)
        && !device.capabilities.contains(DeviceCapabilities::CONTROL)
        && !device
            .capabilities
            .contains(DeviceCapabilities::DFU_RUNTIME_ONLY)
}

pub(super) struct ConnectedDeviceRow {
    hub: Entity<Hub>,
    device: Device,
    manifest: Entity<FirmwareManifestModel>,
    manager: FirmwareUpdateManager,
    is_updating: bool,
    sensor_settings: Entity<SensorSettings>,
    transport_panel: Entity<TransportModePanel>,
    image_viewer: Entity<ImageViewer>,
    /// The rename field; shown only while `editing`.
    name_input: Entity<InputState>,
    editing: bool,
    renamed: bool,
    rename_error: Option<String>,
    rename_pending: bool,
    details_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl ConnectedDeviceRow {
    pub(super) fn new(
        device: Device,
        hub: Entity<Hub>,
        manifest: Entity<FirmwareManifestModel>,
        manager: FirmwareUpdateManager,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let is_updating = manager.is_updating(&device.uuid);
        let name_input = cx.new(|cx| InputState::new(window, cx));
        let sensor_settings = cx.new(|cx| SensorSettings::new(device.clone(), window, cx));
        let transport_panel = cx.new(|cx| TransportModePanel::new(device.clone(), cx));
        let image_viewer = cx.new(|cx| ImageViewer::new(device.clone(), is_updating, cx));
        let (_, ticker): (_, Entity<UpdateTicker>) = FirmwareUpdateManager::global(cx);

        let subscriptions = vec![
            // Battery and the net core reading arrive through the hub.
            cx.observe(&hub, |_, _, cx| cx.notify()),
            cx.observe(&manifest, |_, _, cx| cx.notify()),
            cx.observe(&ticker, |this, _, cx| this.refresh_updating(cx)),
            cx.subscribe_in(&name_input, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_rename(cx);
                }
            }),
        ];
        Self {
            hub,
            device,
            manifest,
            manager,
            is_updating,
            sensor_settings,
            transport_panel,
            image_viewer,
            name_input,
            editing: false,
            renamed: false,
            rename_error: None,
            rename_pending: false,
            details_open: false,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn set_device(&mut self, device: Device, cx: &mut Context<'_, Self>) {
        self.device = device.clone();
        self.sensor_settings
            .update(cx, |panel, cx| panel.set_device(device.clone(), cx));
        self.transport_panel
            .update(cx, |panel, cx| panel.set_device(device.clone(), cx));
        let is_updating = self.is_updating;
        self.image_viewer
            .update(cx, |viewer, cx| viewer.set_state(device, is_updating, cx));
        cx.notify();
    }

    fn refresh_updating(&mut self, cx: &mut Context<'_, Self>) {
        let is_updating = self.manager.is_updating(&self.device.uuid);
        if is_updating != self.is_updating {
            self.is_updating = is_updating;
            let device = self.device.clone();
            self.image_viewer
                .update(cx, |viewer, cx| viewer.set_state(device, is_updating, cx));
        }
        cx.notify();
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let name = self.device.name().to_string();
        self.name_input.update(cx, |input, cx| {
            input.set_value(name, window, cx);
            input.focus(window, cx);
        });
        self.renamed = false;
        self.rename_error = None;
        self.editing = true;
        cx.notify();
    }

    /// One rename is sent at a time, with an owned client handle rather than
    /// anything of the hub held across the RPC.
    fn submit_rename(&mut self, cx: &mut Context<'_, Self>) {
        if !self.editing || self.rename_pending {
            return;
        }
        self.rename_pending = true;
        self.editing = false;
        let name = self.name_input.read(cx).value().to_string();
        let device = self.device.clone();
        let client = self.hub.read(cx).client();
        let rename = runtime::spawn(async move {
            let Some(client) = client else {
                tracing::warn!("Failed to rename device: not connected to the hub");
                return Err("not connected to the hub".to_owned());
            };
            client
                .device(device)
                .set_device_name(name)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to rename device: {e}");
                    e.to_string()
                })
        });
        cx.spawn(async move |this, cx| {
            let outcome = rename
                .await
                .unwrap_or_else(|e| Err(format!("The rename task stopped unexpectedly: {e}")));
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(()) => this.renamed = true,
                    Err(error) => this.rename_error = Some(error),
                }
                this.rename_pending = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn status_dot(&self, p: Palette) -> AnyElement {
        let dot = || div().size_2().rounded_full();
        let id = SharedString::from(format!("dot-{:02x?}", self.device.uuid));
        let pulse = |color| {
            dot().id(id.clone()).bg(color).with_animation(
                id.clone(),
                Animation::new(Duration::from_secs(2))
                    .repeat()
                    .with_easing(pulsating_between(0.4, 1.0)),
                |el, delta| el.opacity(delta),
            )
        };
        if self.is_updating {
            pulse(p.accent).into_any_element()
        } else if is_dfu(&self.device) {
            pulse(p.warning).into_any_element()
        } else {
            dot().bg(p.success).into_any_element()
        }
    }

    /// The device's label, which turns into a text field to rename it.
    fn device_name(&self, p: Palette, cx: &mut Context<'_, Self>) -> AnyElement {
        if self.editing {
            return div()
                .flex()
                .items_center()
                .gap_3()
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.editing = false;
                        cx.notify();
                    }
                }))
                .child(div().w(px(192.0)).child(Input::new(&self.name_input)))
                .child(
                    div()
                        .id("cancel-rename")
                        .cursor_pointer()
                        .text_xs()
                        .text_color(p.fg_muted)
                        .hover(|s| s.text_color(p.fg_strong))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.editing = false;
                            cx.notify();
                        }))
                        .child("✕"),
                )
                .into_any_element();
        }

        let mut name = div().flex().items_center().gap_3().child(
            div()
                .text_sm()
                .font_family("monospace")
                .text_color(p.fg)
                .child(crate::views::device_label(&self.device)),
        );
        if self
            .device
            .capabilities
            .contains(DeviceCapabilities::CONTROL)
        {
            let pending = self.rename_pending;
            let mut pencil = div()
                .id("rename")
                .text_xs()
                .text_color(p.fg_muted)
                .child("✎");
            pencil = if pending {
                pencil.opacity(0.5)
            } else {
                pencil
                    .cursor_pointer()
                    .hover(|s| s.text_color(p.accent_ink))
                    .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx)))
            };
            name = name.child(pencil);
        }
        if self.renamed {
            name = name.child(
                div()
                    .text_xs()
                    .italic()
                    .text_color(p.warning_ink)
                    .child("USB name reflects last boot"),
            );
        }
        if let Some(error) = &self.rename_error {
            name = name.child(
                div()
                    .text_xs()
                    .text_color(p.danger_ink)
                    .child(error.clone()),
            );
        }
        name.into_any_element()
    }
}

impl Render for ConnectedDeviceRow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let device = &self.device;
        let firmware_version = device.firmware_version;
        let battery = self.hub.read(cx).battery_states.get(&device.uuid).copied();

        let firmware_row = div()
            .flex()
            .items_center()
            .gap_2()
            .text_xs()
            .child(div().text_color(p.fg_muted).child("Firmware:"))
            .child(if firmware_version != UNKNOWN_FIRMWARE_VERSION {
                div()
                    .font_family("monospace")
                    .text_color(p.fg)
                    .child(format!(
                        "{}.{}.{}",
                        firmware_version[0], firmware_version[1], firmware_version[2]
                    ))
            } else {
                div().italic().text_color(p.fg_muted).child("Not available")
            });

        let manifest = self.manifest.read(cx).manifest.clone();
        let update = device_firmware_update(
            device,
            manifest.as_ref(),
            &self.manager,
            p,
            self.details_open,
        );

        let mut row = div()
            .relative()
            .flex()
            .flex_col()
            .p_3()
            .gap_2()
            .rounded(px(RADIUS))
            .bg(p.surface_alt)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(self.status_dot(p))
                            .child(self.device_name(p, cx)),
                    )
                    .child(div().text_xs().text_color(p.fg_muted).child(format!(
                        "{}{}",
                        transport_name(device.transport),
                        transport_suffix(device)
                    ))),
            )
            .child(firmware_row);
        if let Some((percent, charging)) = battery {
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(div().text_color(p.fg_muted).child("Battery:"))
                    .child(
                        div()
                            .font_family("monospace")
                            .text_color(p.fg)
                            .child(format!("{percent}%")),
                    )
                    .child(
                        div()
                            .text_color(if charging { p.success_ink } else { p.fg_muted })
                            .child(if charging { "Charging" } else { "Discharging" }),
                    ),
            );
        }
        row = row.child(update).child(
            button(
                ("device-details", cx.entity_id()),
                if self.details_open {
                    "Hide Advanced"
                } else {
                    "Advanced device details"
                },
                ButtonKind::Secondary,
                p,
            )
            .small()
            .on_click(cx.listener(|this, _, _, cx| {
                this.details_open = !this.details_open;
                cx.notify();
            })),
        );
        if self.details_open {
            let mut details = div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_sm().text_color(p.fg).child("Advanced"))
                .child(div().text_xs().text_color(p.fg_muted).child(
                    "Connection, image diagnostics and sensor tuning. Transport changes restart the tracker; save settings only when ready.",
                ));
            if !device
                .capabilities
                .contains(DeviceCapabilities::DFU_RUNTIME_ONLY)
            {
                details = details.child(self.image_viewer.clone());
            }
            row = row.child(
                details
                    .child(self.transport_panel.clone())
                    .child(self.sensor_settings.clone()),
            );
        }
        row
    }
}
