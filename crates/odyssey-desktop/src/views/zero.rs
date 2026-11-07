//! The Zero window: a fullscreen target on the chosen monitor and a sidebar to
//! arm each device's zero.
//!
//! The window is its own root, so it follows the hub through the shared `Hub`
//! entity and the tracking and impact broadcast channels.

mod device_controls;
mod device_state;
mod device_sync;
mod history;
mod save_zero_button;
mod shot_delay_control;
mod zeroing;

use std::collections::HashMap;
use std::sync::Arc;

use device_controls::DeviceControls;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Hsla, Image, ImageFormat,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, WindowKind, div, img,
    px,
};
use history::HistoryByDevice;
use odyssey_hub_common::config::MonitorKey;
use odyssey_hub_common::device::Device;

use crate::components::crosshair_manager::CrosshairManager;
use crate::components::screen_picker::{display_for_key, fullscreen_options};
use crate::hub::{Hub, HubHandle};
use crate::theme::palette;

/// Zero point in screen space. The target layer covers the window, the window
/// is fullscreen on the screen the hub resolves against, and the target image
/// is centered in that layer, so the target sits at the screen center.
const ZERO_TARGET: nalgebra::Vector2<f32> = nalgebra::Vector2::new(0.5, 0.5);

const TARGET_IMAGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/images/target.png"
));
/// Pixel size of [`TARGET_IMAGE`], for its aspect ratio.
const TARGET_IMAGE_SIZE: (f32, f32) = (2021.0, 2481.0);
/// The target is eleven inches wide at CSS's 96 pixels per inch, but never more
/// than this fraction of the window.
const TARGET_MAX_WIDTH: f32 = 11.0 * 96.0;
const TARGET_MAX_WIDTH_FRACTION: f32 = 0.4;

/// Opens the Zero window fullscreen on the display `monitor` identifies.
pub(crate) fn open(monitor: MonitorKey, cx: &mut App) {
    let Some(display) = display_for_key(&monitor, cx) else {
        tracing::warn!("Cannot open Zero: display {monitor:?} is no longer attached");
        return;
    };
    let options = fullscreen_options(display.as_ref(), WindowKind::Normal);
    let opened = cx.open_window(options, |window, cx| cx.new(|cx| ZeroView::new(window, cx)));
    if let Err(error) = opened {
        tracing::error!("Failed to open the Zero window: {error}");
    }
}

struct ZeroView {
    hub: Entity<Hub>,
    devices: Vec<Device>,
    /// Per-device zeroing state, keyed by uuid. List positions are reused as
    /// devices come and go, so keying by position would hand one device the
    /// previous occupant's "zero on shot" flag and shot delay.
    controls: HashMap<[u8; 6], Entity<DeviceControls>>,
    history: HistoryByDevice,
    last_snapshot: Vec<[u8; 6]>,
    crosshairs: Entity<CrosshairManager>,
    target: Arc<Image>,
    focus: FocusHandle,
    _hub_changed: Subscription,
    _history: Task<()>,
    _impacts: Task<()>,
}

impl ZeroView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hub = cx.global::<HubHandle>().0.clone();
        let focus = cx.focus_handle();
        focus.focus(window, cx);

        let hub_changed = cx.observe_in(&hub, window, |zero, _, window, cx| {
            zero.sync_devices(window, cx)
        });
        let history = history::follow_tracking(&hub, cx);
        let impacts = zeroing::zero_on_impact(&hub, cx);
        let crosshairs = cx.new(CrosshairManager::new);

        let mut zero = Self {
            hub,
            devices: Vec::new(),
            controls: HashMap::new(),
            history: HistoryByDevice::new(),
            last_snapshot: Vec::new(),
            crosshairs,
            target: Arc::new(Image::from_bytes(ImageFormat::Png, TARGET_IMAGE.to_vec())),
            focus,
            _hub_changed: hub_changed,
            _history: history,
            _impacts: impacts,
        };
        zero.sync_devices(window, cx);
        zero
    }
}

impl Render for ZeroView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let target_width =
            TARGET_MAX_WIDTH.min(window.viewport_size().width.as_f32() * TARGET_MAX_WIDTH_FRACTION);
        let target_height = target_width * TARGET_IMAGE_SIZE.1 / TARGET_IMAGE_SIZE.0;

        let sidebar = div()
            .id("zero-sidebar")
            .absolute()
            .top_0()
            .left_0()
            .h_full()
            .overflow_y_scroll()
            .px_3()
            .py_4()
            .flex()
            .flex_col()
            .gap_2()
            .bg(Hsla::from(p.surface).opacity(0.8))
            .children(
                self.devices
                    .iter()
                    .filter_map(|device| self.controls.get(&device.uuid).cloned()),
            );

        // Pinned to the window, not laid out next to the sidebar: the target
        // center has to be the screen center, and the crosshair layer maps
        // aimpoints onto this same box.
        div()
            .id("zero-window")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|zero, event: &KeyDownEvent, _, cx| {
                zero.zero_on_key(&event.keystroke.key, cx);
            }))
            .relative()
            .size_full()
            .bg(p.canvas)
            .child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        img(Arc::clone(&self.target))
                            .w(px(target_width))
                            .h(px(target_height)),
                    ),
            )
            .child(div().absolute().size_full().child(self.crosshairs.clone()))
            .child(sidebar)
            .child(
                div()
                    .id("zero-close")
                    .absolute()
                    .top_4()
                    .right_4()
                    .size_10()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(gpui::black().opacity(0.6))
                    .text_color(gpui::white())
                    .text_xl()
                    .cursor_pointer()
                    .hover(|style| style.bg(gpui::black().opacity(0.8)))
                    .on_click(|_, window, _| window.remove_window())
                    .child("✕"),
            )
    }
}
