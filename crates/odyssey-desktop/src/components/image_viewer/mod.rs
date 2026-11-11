//! USB image viewer. A live session exclusively owns its stream. Stop, window
//! close, row removal, or a lost USB link drops it and cancels the RPC. Modern
//! tracking resumes. Legacy PAJ7025 requires reboot/reconnect.
//! Camera calibration deliberately remains in odyssey-vision-tool.

mod dialog;
mod frame;
mod session;

use gpui::component::Sizable as _;
use gpui::component::button::Button;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyWindowHandle, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowOptions, div, px, size,
};
use odyssey_hub_common::device::Device;

use self::dialog::ImageDialog;
use crate::theme::palette;

const DIALOG_WIDTH: f32 = 820.0;
const DIALOG_HEIGHT: f32 = 760.0;

/// The "Image / IR" button of one device row. The capture itself runs in a
/// window of its own.
pub(crate) struct ImageViewer {
    device: Device,
    is_updating: bool,
    key: String,
    needs_reboot: Entity<bool>,
    /// Weak, so closing the window ends the capture session with it.
    dialog: Option<(AnyWindowHandle, WeakEntity<ImageDialog>)>,
}

impl ImageViewer {
    pub(crate) fn new(device: Device, is_updating: bool, cx: &mut Context<Self>) -> Self {
        let needs_reboot = cx.new(|_| false);
        cx.observe(&needs_reboot, |_, _, cx| cx.notify()).detach();
        // The row is gone, so capture must not keep running without it.
        cx.on_release(|this, cx| {
            if let Some((window, _)) = this.dialog.take() {
                let _ = window.update(cx, |_, window, _| window.remove_window());
            }
        })
        .detach();
        Self {
            key: hex::encode(device.uuid),
            device,
            is_updating,
            needs_reboot,
            dialog: None,
        }
    }

    pub(crate) fn set_state(&mut self, device: Device, is_updating: bool, cx: &mut Context<Self>) {
        if self.device == device && self.is_updating == is_updating {
            return;
        }
        self.device = device.clone();
        self.is_updating = is_updating;
        if let Some((_, dialog)) = &self.dialog
            && dialog
                .update(cx, |dialog, cx| dialog.set_state(device, is_updating, cx))
                .is_err()
        {
            self.dialog = None;
        }
        cx.notify();
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        if let Some((window, dialog)) = &self.dialog
            && dialog.upgrade().is_some()
            && window
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(
                    format!("Image / IR — {}", crate::views::device_label(&self.device)).into(),
                ),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(DIALOG_WIDTH), px(DIALOG_HEIGHT)),
                cx,
            ))),
            ..Default::default()
        };
        let device = self.device.clone();
        let is_updating = self.is_updating;
        let needs_reboot = self.needs_reboot.clone();
        match gpui::open_window(options, cx, |window, cx| {
            cx.new(|cx| ImageDialog::new(device, is_updating, needs_reboot, window, cx))
        }) {
            Ok((window, dialog)) => self.dialog = Some((window, dialog.downgrade())),
            Err(error) => tracing::error!("Could not open the image window: {error:#}"),
        }
    }
}

impl Render for ImageViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Keep the action reachable even over BLE so the USB requirement is visible.
        let known_sensor = matches!(self.device.product_id, 0x520f..=0x5211);
        if !known_sensor {
            return div();
        }
        let palette = &palette(cx);
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Button::new(SharedString::from(format!("image-ir-{}", self.key)))
                    .label("Image / IR")
                    .small()
                    .outline()
                    .on_click(cx.listener(|this, _, _, cx| this.open(cx))),
            )
            .when(*self.needs_reboot.read(cx), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(palette.warning_ink)
                        .child("Legacy image mode: reboot/reconnect required"),
                )
            })
    }
}
