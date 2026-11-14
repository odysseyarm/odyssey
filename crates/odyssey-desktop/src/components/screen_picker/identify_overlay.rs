use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Task, Window, WindowKind, div, px,
    relative,
};

use super::monitor_layout::MonitorInfo;
use super::monitor_selection::{fullscreen_options, monitor_key_of};
use crate::theme::palette;

/// How long the numbered overlay stays on each monitor.
pub(super) const IDENTIFY_DURATION: Duration = Duration::from_millis(1800);

/// Puts a numbered overlay on every physical monitor for a moment. The numbers
/// match the ones on the picker's tiles because both follow the OS's order.
pub(super) fn identify_monitors(cx: &mut App) {
    for (index, display) in cx.displays().into_iter().enumerate() {
        let info = MonitorInfo {
            key: monitor_key_of(display.as_ref()),
            primary: false,
        };
        let number = index + 1;
        let label = format!("{} · {}", info.display_name(), info.resolution());
        let options = fullscreen_options(display.as_ref(), WindowKind::Normal);
        let opened = cx.open_window(options, |window, cx| {
            cx.new(|cx| IdentifyOverlay::new(number, label.into(), window, cx))
        });
        if let Err(error) = opened {
            tracing::warn!("Failed to open the identify overlay on display {number}: {error}");
        }
    }
}

struct IdentifyOverlay {
    number: usize,
    label: SharedString,
    _close: Task<()>,
}

impl IdentifyOverlay {
    fn new(number: usize, label: SharedString, window: &Window, cx: &mut Context<Self>) -> Self {
        let timer = cx.background_executor().timer(IDENTIFY_DURATION);
        let close = cx.spawn_in(window, async move |_, cx| {
            timer.await;
            let _ = cx.update(|window, _| window.remove_window());
        });
        Self {
            number,
            label,
            _close: close,
        }
    }
}

impl Render for IdentifyOverlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &palette(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_6()
            .bg(p.accent)
            .text_color(p.on_accent)
            .cursor_pointer()
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.remove_window();
            })
            .child(
                div()
                    .text_size(px(256.0))
                    .line_height(relative(1.0))
                    .font_weight(FontWeight::BOLD)
                    .child(self.number.to_string()),
            )
            .child(div().text_2xl().opacity(0.9).child(self.label.clone()))
    }
}
