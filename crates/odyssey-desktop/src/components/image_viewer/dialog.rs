use std::time::{Duration, Instant};

use gpui::component::button::Button;
use gpui::component::select::{Select, SelectEvent, SelectState};
use gpui::component::{Disableable as _, IndexPath, Sizable as _};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div, px,
};
use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport};
use odyssey_protocol::Port;

use super::frame::{PresentedFrame, frame_view};
use super::session::{LiveSession, SessionEvent};
use crate::hub::HubHandle;
use crate::runtime;
use crate::theme::palette;

const MAX_FPS: u8 = 15;
const CAMERA_OPTIONS: [&str; 2] = ["NF — near field", "WF — wide field"];

fn unavailable(device: &Device, is_updating: bool) -> Option<&'static str> {
    if is_updating {
        Some("Image capture is unavailable during a firmware update.")
    } else if device.capabilities.contains(DeviceCapabilities::DFU)
        && !device.capabilities.contains(DeviceCapabilities::CONTROL)
    {
        Some("Image capture requires normal application firmware, not bootloader mode.")
    } else if !device.capabilities.contains(DeviceCapabilities::CONTROL)
        || !device.capabilities.contains(DeviceCapabilities::EVENTS)
        || device.events_transport != EventsTransport::Wired
    {
        Some(
            "USB only: connect this device directly by USB and select USB sensor events. BLE and dongle links cannot carry images.",
        )
    } else if !device.events_connected {
        Some("The USB sensor connection is not available. Capture has stopped.")
    } else {
        None
    }
}

/// Legacy PAJ7025 sensors lose tracking in image mode until a reboot.
fn is_legacy(device: &Device) -> bool {
    matches!(device.product_id, 0x520f | 0x5210)
}

/// The live image window of one device.
pub(super) struct ImageDialog {
    device: Device,
    is_updating: bool,
    /// Shared with the viewer: legacy image mode outlives the window.
    needs_reboot: Entity<bool>,
    camera: Port,
    fps: u8,
    running: bool,
    status: String,
    error: Option<String>,
    latest: Option<PresentedFrame>,
    rebooting: bool,
    /// Availability at the last device update, so a loss is handled once.
    could_stream: Option<bool>,
    session: Option<LiveSession>,
    camera_select: Entity<SelectState<Vec<SharedString>>>,
    fps_select: Entity<SelectState<Vec<SharedString>>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _clock: Task<()>,
}

impl ImageDialog {
    pub(super) fn new(
        device: Device,
        is_updating: bool,
        needs_reboot: Entity<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let supports_wf = is_legacy(&device);
        let camera_options: Vec<SharedString> = CAMERA_OPTIONS
            .iter()
            .take(if supports_wf { 2 } else { 1 })
            .map(|&name| SharedString::from(name))
            .collect();
        let camera_select = cx.new(|cx| {
            SelectState::new(
                camera_options,
                Some(IndexPath::default().row(0)),
                window,
                cx,
            )
        });
        let fps_options: Vec<SharedString> = (1..=MAX_FPS)
            .map(|value| SharedString::from(value.to_string()))
            .collect();
        let fps = MAX_FPS;
        let fps_select = cx.new(|cx| {
            SelectState::new(
                fps_options,
                Some(IndexPath::default().row(usize::from(fps) - 1)),
                window,
                cx,
            )
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &camera_select,
                window,
                |this, _, event: &SelectEvent<Vec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(chosen)) = event {
                        this.camera = if chosen.as_ref() == CAMERA_OPTIONS[1] {
                            Port::Wf
                        } else {
                            Port::Nf
                        };
                        this.set_latest(None, window);
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &fps_select,
                window,
                |this, _, event: &SelectEvent<Vec<SharedString>>, _, cx| {
                    if let SelectEvent::Confirm(Some(chosen)) = event
                        && let Ok(value) = chosen.parse::<u8>()
                    {
                        this.fps = value;
                        cx.notify();
                    }
                },
            ),
        ];
        // Updates receive age even when no new frames arrive or the view is paused.
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut dialog = Self {
            device,
            is_updating,
            needs_reboot,
            camera: Port::Nf,
            fps,
            running: false,
            status: "Stopped.".to_owned(),
            error: None,
            latest: None,
            rebooting: false,
            could_stream: None,
            session: None,
            camera_select,
            fps_select,
            focus,
            _subscriptions: subscriptions,
            _clock: clock,
        };
        dialog.sync_availability();
        dialog
    }

    /// Follows the device as the hub reports it.
    pub(super) fn set_state(&mut self, device: Device, is_updating: bool, cx: &mut Context<Self>) {
        self.device = device;
        self.is_updating = is_updating;
        self.sync_availability();
        cx.notify();
    }

    /// Stops capture when the USB image connection goes away.
    fn sync_availability(&mut self) {
        let can_stream = unavailable(&self.device, self.is_updating).is_none();
        if self.could_stream == Some(can_stream) {
            return;
        }
        self.could_stream = Some(can_stream);
        if !can_stream {
            self.running = false;
            self.session = None;
            if is_legacy(&self.device) {
                "USB unavailable. Legacy image mode still requires reboot/power-cycle and reconnect."
            } else {
                "Stopped: the USB image connection is unavailable."
            }
            .clone_into(&mut self.status);
        }
    }

    fn set_latest(&mut self, frame: Option<PresentedFrame>, window: &mut Window) {
        if let Some(old) = std::mem::replace(&mut self.latest, frame) {
            // The atlas keeps a texture per image until told otherwise.
            let _ = window.drop_image(old.image);
        }
    }

    pub(super) fn on_session_event(
        &mut self,
        event: SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let legacy = is_legacy(&self.device);
        match event {
            SessionEvent::Started => {
                if legacy {
                    "Capturing legacy USB images. Tracking is stopped until reboot/reconnect."
                } else {
                    "Capturing USB images. Tracking paused until Stop or close."
                }
                .clone_into(&mut self.status);
            }
            SessionEvent::StartFailed(cause) => {
                self.error = Some(format!("Could not start USB image capture: {cause}"));
                if legacy {
                    "No preview active. Legacy startup may have changed sensor mode: reboot/power-cycle and reconnect before tracking."
                } else {
                    "Stopped; no image session is active."
                }
                .clone_into(&mut self.status);
                self.running = false;
                self.session = None;
            }
            SessionEvent::Frame(frame) => self.set_latest(Some(frame), window),
            SessionEvent::Ended(failure) => {
                self.error = Some(failure);
                if legacy {
                    "Pixels stopped. Legacy tracking is NOT restored; reboot/power-cycle and reconnect."
                } else {
                    "Stopped. Any displayed image is the last received frame."
                }
                .clone_into(&mut self.status);
                self.running = false;
                self.session = None;
            }
        }
        cx.notify();
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_latest(None, window);
        self.error = None;
        "Starting USB image session…".clone_into(&mut self.status);
        let legacy = is_legacy(&self.device);
        if legacy {
            self.needs_reboot.update(cx, |needs_reboot, cx| {
                *needs_reboot = true;
                cx.notify();
            });
        }
        self.running = true;
        let client = cx.global::<HubHandle>().0.read(cx).client();
        self.session = Some(LiveSession::start(
            client,
            self.device.clone(),
            self.camera,
            self.fps,
            window,
            cx,
        ));
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.running = false;
        self.session = None;
        if is_legacy(&self.device) {
            "Pixels stopped. Legacy tracking is NOT restored: reboot/power-cycle and reconnect."
        } else {
            "Stopped. Last received frame is frozen; normal tracking is being restored."
        }
        .clone_into(&mut self.status);
        cx.notify();
    }

    fn reboot(&mut self, cx: &mut Context<Self>) {
        self.running = false;
        self.session = None;
        self.rebooting = true;
        "Stopping pixels and requesting legacy reboot; waiting for reconnect is required."
            .clone_into(&mut self.status);
        cx.notify();
        let device = self.device.clone();
        let client = cx.global::<HubHandle>().0.read(cx).client();
        cx.spawn(async move |this, cx| {
            let result = runtime::spawn(async move {
                match client {
                    Some(client) => client
                        .device(device)
                        .reboot_device()
                        .await
                        .map_err(|cause| cause.to_string()),
                    None => Err("not connected to the hub".to_owned()),
                }
            })
            .await
            .unwrap_or_else(|cause| Err(cause.to_string()));
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => "Reboot acknowledged. Wait for the device to reconnect; acknowledgement alone does not confirm tracking recovery.".clone_into(&mut this.status),
                    Err(cause) => this.error = Some(format!("Reboot failed: {cause}. Fully power-cycle the unit and reconnect.")),
                }
                this.rebooting = false;
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for ImageDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = &palette(cx);
        let reason = unavailable(&self.device, self.is_updating);
        let can_stream = reason.is_none();
        let legacy = is_legacy(&self.device);
        let needs_reboot = *self.needs_reboot.read(cx);
        let help = |text: SharedString| div().text_sm().text_color(palette.fg_muted).child(text);
        let labelled = |label: &'static str, control: gpui::AnyElement| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(help(label.into()))
                .child(control)
        };

        let toggle = if self.running {
            Button::new("image-stop")
                .label("Stop")
                .small()
                .outline()
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        } else {
            Button::new("image-start")
                .label(if legacy {
                    "Enter legacy images (reboot required)"
                } else {
                    "Start"
                })
                .small()
                .outline()
                .disabled(!can_stream || needs_reboot || self.rebooting)
                .on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))
        };

        div()
            .id("image-dialog")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            }))
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .bg(palette.surface)
            .text_color(palette.fg)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("Image / IR — {}", crate::views::device_label(&self.device))),
                    )
                    .child(
                        Button::new("image-close")
                            .label("Close")
                            .small()
                            .outline()
                            .on_click(|_, window, _| window.remove_window()),
                    ),
            )
            .child(help(if legacy {
                "Legacy PAJ7025: native 98×98 NF/WF images. Entering image mode stops tracking and IMU reports. Stop, close, error or disconnect stops pixels ONLY; reboot/power-cycle and reconnect are REQUIRED to restore tracking or switch camera. Saved settings are unchanged; unsaved sensor edits must be reapplied after reboot."
            } else {
                "Image-only capture. Tracking pauses until Stop or close; IMU/control remain available."
            }.into()))
            .children(reason.map(|reason| {
                div().text_sm().text_color(palette.danger_ink).child(reason)
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .flex_wrap()
                    .child(labelled(
                        "Camera",
                        Select::new(&self.camera_select)
                            .small()
                            .w(px(180.0))
                            .disabled(self.running)
                            .into_any_element(),
                    ))
                    .child(labelled(
                        "Requested FPS",
                        Select::new(&self.fps_select)
                            .small()
                            .w(px(80.0))
                            .disabled(self.running)
                            .into_any_element(),
                    ))
                    .child(toggle),
            )
            .when(needs_reboot, |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(palette.warning_ink)
                        .child("Tracking has not been restored. Reboot then wait for a new USB connection before using tracking/settings. If reboot is unavailable or recovery fails, fully power-cycle the device (disconnecting USB alone may leave a battery-powered unit on)."),
                )
                .child(
                    div().child(
                        Button::new("image-reboot")
                            .label("Stop images & reboot legacy device")
                            .small()
                            .outline()
                            .disabled(
                                self.rebooting
                                    || !self.device.capabilities.contains(DeviceCapabilities::CONTROL),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.reboot(cx))),
                    ),
                )
            })
            .child(help(self.status.clone().into()))
            .children(self.error.clone().map(|message| {
                div().text_sm().text_color(palette.danger_ink).child(message)
            }))
            .child(match &self.latest {
                Some(frame) => frame_view(
                    frame,
                    Instant::now(),
                    self.running && can_stream,
                    palette,
                )
                .into_any_element(),
                None => help("No image received.".into()).into_any_element(),
            })
    }
}
