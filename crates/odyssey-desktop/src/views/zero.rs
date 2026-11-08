//! Fullscreen, selected-tracker alignment task. Target coordinates stay screen-relative.

mod device_controls;
mod device_state;
mod device_sync;
mod history;
mod shot_delay_control;
mod zeroing;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use device_controls::DeviceControls;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Global, Image, ImageFormat,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, WindowHandle,
    WindowKind, div, img, px, relative,
};
use history::HistoryByDevice;
use odyssey_hub_common::config::MonitorKey;
use odyssey_hub_common::device::{Device, DeviceCapabilities};
use odyssey_hub_common::events::TrackingEvent;

use crate::components::screen_picker::{display_for_key, fullscreen_options};
use crate::components::ui::{AlertKind, ButtonKind, alert, button, card_title};
use crate::hub::{Hub, HubHandle};
use crate::theme::palette;
use crate::workspace::{Workspace, WorkspaceHandle};

/// Target and live aim use the same full-window coordinate space, not sidebar bounds.
const ZERO_TARGET: nalgebra::Vector2<f32> = nalgebra::Vector2::new(0.5, 0.5);
const TARGET_IMAGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/images/target.png"
));
const TARGET_IMAGE_SIZE: (f32, f32) = (2021.0, 2481.0);
const TARGET_MAX_WIDTH: f32 = 11.0 * 96.0;
const TARGET_MAX_WIDTH_FRACTION: f32 = 0.4;
const TRACKING_MAX_AGE: Duration = Duration::from_millis(500);

struct ZeroWindow(
    Option<WindowHandle<ZeroView>>,
    HashMap<[u8; 6], Entity<DeviceControls>>,
);
impl Global for ZeroWindow {}

pub(crate) fn open(monitor: MonitorKey, cx: &mut App) {
    if let Some(existing) = cx.try_global::<ZeroWindow>().and_then(|state| state.0)
        && existing
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let Some(display) = display_for_key(&monitor, cx) else {
        tracing::warn!("Cannot open Zero: display {monitor:?} is no longer attached");
        return;
    };
    let options = fullscreen_options(display.as_ref(), WindowKind::Normal);
    match cx.open_window(options, |window, cx| {
        cx.new(|cx| ZeroView::new(monitor, window, cx))
    }) {
        Ok(window) => cx.set_global(ZeroWindow(Some(window), HashMap::new())),
        Err(error) => tracing::error!("Failed to open the Zero window: {error}"),
    }
}

struct ZeroView {
    hub: Entity<Hub>,
    workspace: Entity<Workspace>,
    monitor: MonitorKey,
    devices: Vec<Device>,
    controls: HashMap<[u8; 6], Entity<DeviceControls>>,
    selected_device: Option<[u8; 6]>,
    history: HistoryByDevice,
    tracking: HashMap<[u8; 6], (TrackingEvent, Instant)>,
    target: Arc<Image>,
    focus: FocusHandle,
    exit_confirmation: bool,
    _hub_changed: Subscription,
    _workspace_changed: Subscription,
    control_changes: Vec<Subscription>,
    history_task: Option<Task<()>>,
    tracking_screen: tokio::sync::watch::Sender<Option<u8>>,
    _impacts: Task<()>,
    _readiness: Task<()>,
}

impl ZeroView {
    fn new(monitor: MonitorKey, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hub = cx.global::<HubHandle>().0.clone();
        let workspace = cx.global::<WorkspaceHandle>().0.clone();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |zero, cx| zero.request_close(window, cx));
                false
            } else {
                cx.defer(crate::shell::activate_main);
                true
            }
        });
        let hub_changed = cx.observe_in(&hub, window, |zero, _, window, cx| {
            zero.sync_devices(window, cx);
        });
        let workspace_changed = cx.observe(&workspace, |zero, _, cx| {
            zero.sync_selection(cx);
            zero.refresh_readiness(cx);
            cx.notify();
        });
        let (tracking_screen, screen) = tokio::sync::watch::channel(None);
        let history = history::follow_tracking(screen, cx);
        let impacts = zeroing::zero_on_impact(&hub, cx);
        let readiness = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if this.update(cx, Self::refresh_readiness).is_err() {
                    break;
                }
            }
        });
        let controls = if cx.has_global::<ZeroWindow>() {
            std::mem::take(&mut cx.global_mut::<ZeroWindow>().1)
        } else {
            HashMap::new()
        };
        let mut zero = Self {
            hub,
            workspace,
            monitor,
            devices: Vec::new(),
            controls,
            selected_device: None,
            history: HistoryByDevice::new(),
            tracking: HashMap::new(),
            target: Arc::new(Image::from_bytes(ImageFormat::Png, TARGET_IMAGE.to_vec())),
            focus,
            exit_confirmation: false,
            _hub_changed: hub_changed,
            _workspace_changed: workspace_changed,
            control_changes: Vec::new(),
            history_task: Some(history),
            tracking_screen,
            _impacts: impacts,
            _readiness: readiness,
        };
        for controls in zero.controls.values() {
            controls.update(cx, |controls, cx| controls.rebind_window(window, cx));
            zero.control_changes
                .push(cx.observe(controls, |zero, _, cx| {
                    zero.refresh_readiness(cx);
                    cx.notify();
                }));
        }
        zero.sync_devices(window, cx);
        zero
    }

    fn needs_exit_warning(&self, cx: &App) -> bool {
        self.controls.values().any(|controls| {
            let controls = controls.read(cx);
            controls.state.needs_exit_warning() || controls.pending
        })
    }

    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for controls in self.controls.values() {
            controls.update(cx, |controls, cx| {
                controls.state.cancel("Waiting for shot cancelled.");
                cx.notify();
            });
        }
        if self.needs_exit_warning(cx) {
            self.exit_confirmation = true;
            self.focus.focus(window, cx);
            cx.notify();
        } else {
            self.leave(window, cx);
        }
    }

    fn leave(&mut self, window: &mut Window, cx: &mut App) {
        self.history_task.take();
        cx.set_global(ZeroWindow(None, std::mem::take(&mut self.controls)));
        window.remove_window();
        cx.defer(crate::shell::activate_main);
    }

    fn capture_now(&mut self, cx: &mut Context<Self>) {
        if self.exit_confirmation {
            return;
        }
        self.refresh_readiness(cx);
        if let Some(controls) = self
            .selected_device
            .and_then(|uuid| self.controls.get(&uuid))
            .cloned()
        {
            controls.update(cx, |controls, cx| {
                controls.state.arm(controls.ready && !controls.pending);
                cx.notify();
            });
            self.capture_selected(None, cx);
        }
    }

    fn key_down(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if key == "Escape" {
            if self.exit_confirmation {
                self.exit_confirmation = false;
                cx.notify();
            } else if let Some(controls) = self
                .selected_device
                .and_then(|uuid| self.controls.get(&uuid))
                .cloned()
                && controls.read(cx).state.armed
            {
                controls.update(cx, |controls, cx| {
                    controls
                        .state
                        .cancel("Waiting for shot cancelled. No zero captured.");
                    cx.notify();
                });
            } else {
                self.request_close(window, cx);
            }
        } else if key.eq_ignore_ascii_case("z") && self.focus.is_focused(window) {
            self.capture_now(cx);
        }
    }
}

impl Render for ZeroView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let target_width =
            TARGET_MAX_WIDTH.min(window.viewport_size().width.as_f32() * TARGET_MAX_WIDTH_FRACTION);
        let target_height = target_width * TARGET_IMAGE_SIZE.1 / TARGET_IMAGE_SIZE.0;
        let selected = self
            .selected_device
            .and_then(|uuid| self.controls.get(&uuid))
            .cloned();
        let (ready, busy, changed) = selected.as_ref().map_or((false, false, false), |controls| {
            let controls = controls.read(cx);
            (
                controls.ready,
                controls.pending || controls.state.pending.is_some(),
                controls.state.unsaved,
            )
        });
        let mut sidebar = div().id("zero-sidebar").absolute().top_0().left_0().h_full()
            .w(px(320.)).overflow_y_scroll().p_4().flex().flex_col().gap_3()
            .bg(p.surface).text_color(p.fg)
            .child(card_title(p, "Align tracker"))
            .child(div().text_sm().text_color(p.fg_muted).child("Aim at the center, press Z once, verify, then save. This target is fullscreen, not a desktop overlay."))
            .child(button("zero-return", "Return to workspace", ButtonKind::Ghost, p).small()
                .on_click(cx.listener(|zero, _, window, cx| zero.request_close(window, cx))));
        for device in self
            .devices
            .iter()
            .filter(|device| device.capabilities.contains(DeviceCapabilities::EVENTS))
        {
            let uuid = device.uuid;
            sidebar = sidebar.child(
                button(
                    (
                        "zero-tracker",
                        u64::from_le_bytes([
                            uuid[0], uuid[1], uuid[2], uuid[3], uuid[4], uuid[5], 0, 0,
                        ]),
                    ),
                    format!(
                        "{}{}",
                        if self.selected_device == Some(uuid) {
                            "Selected · "
                        } else {
                            "Use · "
                        },
                        crate::views::device_label(device)
                    ),
                    if self.selected_device == Some(uuid) {
                        ButtonKind::Secondary
                    } else {
                        ButtonKind::Ghost
                    },
                    p,
                )
                .small()
                .on_click(cx.listener(move |zero, _, window, cx| {
                    zero.workspace
                        .update(cx, |workspace, cx| workspace.select_device(uuid, cx));
                    zero.sync_selection(cx);
                    zero.refresh_readiness(cx);
                    zero.focus.focus(window, cx);
                })),
            );
        }
        for (&uuid, controls) in &self.controls {
            if self.selected_device != Some(uuid) && controls.read(cx).state.needs_exit_warning() {
                sidebar = sidebar.child(
                    button(
                        (
                            "zero-unsaved-tracker",
                            u64::from_le_bytes([
                                uuid[0], uuid[1], uuid[2], uuid[3], uuid[4], uuid[5], 0, 0,
                            ]),
                        ),
                        format!(
                            "Unsaved / pending · {}",
                            crate::views::device_label(&controls.read(cx).device)
                        ),
                        ButtonKind::Secondary,
                        p,
                    )
                    .small()
                    .on_click(cx.listener(move |zero, _, _, cx| {
                        zero.workspace
                            .update(cx, |workspace, cx| workspace.select_device(uuid, cx));
                        zero.sync_selection(cx);
                        zero.refresh_readiness(cx);
                    })),
                );
            }
        }
        sidebar = sidebar.child(alert(
            if ready {
                AlertKind::Success
            } else {
                AlertKind::Info
            },
            p,
            div().child(self.readiness_message(cx)),
        ));
        if let Some(controls) = selected {
            sidebar = sidebar
                .child(
                    button(
                        "zero-capture",
                        if changed {
                            "Capture again [Z]"
                        } else {
                            "Capture now [Z]"
                        },
                        if changed {
                            ButtonKind::Secondary
                        } else {
                            ButtonKind::Primary
                        },
                        p,
                    )
                    .disabled(!ready || busy || self.exit_confirmation)
                    .on_click(cx.listener(|zero, _, window, cx| {
                        zero.focus.focus(window, cx);
                        zero.capture_now(cx);
                    })),
                )
                .child(controls);
        }
        let mut surface = div()
            .id("zero-window")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|zero, event: &KeyDownEvent, window, cx| {
                if !event.is_held {
                    zero.key_down(&event.keystroke.key, window, cx);
                }
            }))
            .relative()
            .size_full()
            .bg(p.canvas)
            .child(
                div()
                    .id("zero-target")
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_click(cx.listener(|zero, _, window, cx| zero.focus.focus(window, cx)))
                    .child(
                        img(Arc::clone(&self.target))
                            .w(px(target_width))
                            .h(px(target_height)),
                    ),
            );
        if let Some((event, _)) = self
            .selected_device
            .and_then(|uuid| self.tracking.get(&uuid))
            && ready
        {
            surface = surface.child(
                div()
                    .absolute()
                    .left(relative(event.aimpoint.x.clamp(0., 1.)))
                    .top(relative(event.aimpoint.y.clamp(0., 1.)))
                    .child(
                        div()
                            .absolute()
                            .left(px(-14.))
                            .top(px(-1.))
                            .w(px(28.))
                            .h(px(2.))
                            .bg(p.danger),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(-1.))
                            .top(px(-14.))
                            .w(px(2.))
                            .h(px(28.))
                            .bg(p.danger),
                    ),
            );
        }
        surface = surface.child(sidebar);
        if self.exit_confirmation {
            surface = surface.child(div().absolute().size_full().flex().items_center().justify_center()
                .bg(gpui::black().opacity(0.75))
                .child(div().w(px(480.)).p_5().rounded_xl().bg(p.surface).text_color(p.fg).flex().flex_col().gap_3()
                    .child(card_title(p, "Leave alignment task?"))
                    .child("Changes are unsaved, result is unknown, or a command is still running. Leaving does not cancel an already queued command. Stay to verify and save; reconnect first if needed.")
                    .child(button("zero-stay", "Stay and finish", ButtonKind::Primary, p)
                        .on_click(cx.listener(|zero, _, window, cx| { zero.exit_confirmation = false; zero.focus.focus(window, cx); cx.notify(); })))
                    .child(button("zero-leave", "Leave without saving", ButtonKind::Danger, p)
                        .on_click(cx.listener(|zero, _, window, cx| zero.leave(window, cx))))));
        }
        surface
    }
}
