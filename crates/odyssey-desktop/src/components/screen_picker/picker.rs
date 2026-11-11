use std::time::Duration;

use gpui::{
    Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, px,
    relative,
};

use super::identify_overlay::{IDENTIFY_DURATION, identify_monitors};
use super::monitor_layout::{Detail, MonitorInfo, Tile, detail_for, layout};
use super::monitor_selection::MonitorSelection;
use crate::components::ui::{ButtonKind, button};
use crate::theme::{Palette, RADIUS, palette};

/// Widest and tallest the diagram grows, in pixels. The diagram shrinks below
/// the width when its container is narrower.
const DIAGRAM_MAX_WIDTH: f64 = 416.0;
const DIAGRAM_MAX_HEIGHT: f64 = 200.0;

/// Keeps the Identify button disabled a moment past the overlay, so overlays
/// never stack.
const IDENTIFY_COOLDOWN: Duration = Duration::from_millis(400);

pub(crate) struct ScreenPicker {
    selection: Entity<MonitorSelection>,
    identifying: bool,
    identify_reset: Option<Task<()>>,
    _selection_changed: Subscription,
}

impl ScreenPicker {
    pub(crate) fn new(
        selection: Entity<MonitorSelection>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let selection_changed = cx.observe(&selection, |_, _, cx| cx.notify());
        Self {
            selection,
            identifying: false,
            identify_reset: None,
            _selection_changed: selection_changed,
        }
    }

    fn identify(&mut self, cx: &mut Context<Self>) {
        self.identifying = true;
        identify_monitors(cx);
        let timer = cx
            .background_executor()
            .timer(IDENTIFY_DURATION + IDENTIFY_COOLDOWN);
        self.identify_reset = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |picker, cx| {
                picker.identifying = false;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for ScreenPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &palette(cx);
        let (monitors, selected) = {
            let selection = self.selection.read(cx);
            (selection.monitors().to_vec(), selection.selected())
        };
        let diagram = layout(&monitors, DIAGRAM_MAX_WIDTH, DIAGRAM_MAX_HEIGHT);

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.fg_muted)
                    .child("Choose a display:"),
            )
            .child(
                button("identify-monitors", "Identify", ButtonKind::Secondary, p)
                    .small()
                    .disabled(monitors.is_empty() || self.identifying)
                    .on_click(cx.listener(|picker, _, _, cx| picker.identify(cx))),
            );

        let body =
            match diagram {
                Some(diagram) => {
                    let tiles = monitors.iter().zip(&diagram.tiles).enumerate().map(
                        |(i, (monitor, tile))| {
                            let selection = self.selection.clone();
                            monitor_tile(
                                i,
                                monitor,
                                tile,
                                diagram.width,
                                diagram.height,
                                selected == Some(i),
                                p,
                                move |cx| {
                                    selection.update(cx, |selection, cx| selection.select(i, cx));
                                },
                            )
                        },
                    );
                    let diagram_box = div()
                        .flex()
                        .justify_center()
                        .rounded(px(RADIUS))
                        .bg(p.surface_alt)
                        .p_3()
                        .child(
                            div()
                                .relative()
                                .w(px(diagram.width as f32))
                                .max_w(relative(1.0))
                                .aspect_ratio((diagram.width / diagram.height) as f32)
                                .children(tiles),
                        );

                    // A tile too small for its name and resolution gets them spelled out here.
                    let selected_detail = selected
                        .filter(|&i| {
                            diagram
                                .tiles
                                .get(i)
                                .is_some_and(|tile| detail_for(tile) != Detail::Full)
                        })
                        .and_then(|i| monitors.get(i).map(|m| (i, m)))
                        .map(|(i, monitor)| {
                            let primary = if monitor.primary { " — Primary" } else { "" };
                            div()
                                .text_sm()
                                .text_color(p.fg_muted)
                                .text_center()
                                .child(format!(
                                    "{}. {} — {}{primary}",
                                    i + 1,
                                    monitor.display_name(),
                                    monitor.resolution()
                                ))
                        });
                    let primary_legend = monitors
                        .iter()
                        .zip(&diagram.tiles)
                        .any(|(m, t)| m.primary && detail_for(t) != Detail::Full)
                        .then(|| {
                            div()
                                .text_xs()
                                .text_color(p.fg_muted)
                                .text_center()
                                .child("★ primary display")
                        });
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(diagram_box)
                        .children(selected_detail)
                        .children(primary_legend)
                }
                None => div()
                    .rounded(px(RADIUS))
                    .bg(p.surface_alt)
                    .p_6()
                    .text_center()
                    .text_sm()
                    .text_color(p.fg_muted)
                    .child("No displays found."),
            };

        div().flex().flex_col().gap_3().child(header).child(body)
    }
}

/// One clickable monitor in the diagram. `select` runs when it is clicked.
#[expect(
    clippy::too_many_arguments,
    reason = "a tile is exactly its monitor, geometry, state and handler"
)]
fn monitor_tile(
    index: usize,
    monitor: &MonitorInfo,
    tile: &Tile,
    diagram_width: f64,
    diagram_height: f64,
    selected: bool,
    p: &Palette,
    select: impl Fn(&mut gpui::App) + 'static,
) -> impl IntoElement {
    let detail = detail_for(tile);
    let number = index + 1;
    let (bg, border, fg, hover_bg, hover_border) = if selected {
        (
            p.accent,
            p.accent_strong,
            p.on_accent,
            p.accent_strong,
            p.accent_strong,
        )
    } else {
        (
            p.surface,
            p.line_strong,
            p.fg,
            p.surface_hover,
            p.accent_ink,
        )
    };
    let badge_bg: Hsla = if selected {
        gpui::black().opacity(0.15)
    } else {
        p.surface_alt.into()
    };
    let badge_fg = if selected { p.on_accent } else { p.fg_strong };
    let fg_hsla: Hsla = fg.into();

    let mut content = div()
        .id(("monitor-tile", index))
        .relative()
        .size_full()
        .overflow_hidden()
        .rounded(px(RADIUS))
        .border_2()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .px_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_0p5()
        .text_center()
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg).border_color(hover_border))
        .on_click(move |_, _, cx| select(cx))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size_6()
                .flex_shrink_0()
                .rounded_full()
                .bg(badge_bg)
                .text_color(badge_fg)
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .child(number.to_string()),
        );
    if selected {
        content = content.shadow_md();
    }
    if matches!(detail, Detail::Named | Detail::Full) {
        content = content.child(
            div()
                .max_w_full()
                .truncate()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .child(monitor.display_name().to_owned()),
        );
    }
    if detail != Detail::Minimal {
        content = content.child(
            div()
                .max_w_full()
                .truncate()
                .text_size(px(11.0))
                .opacity(0.8)
                .child(monitor.resolution()),
        );
    }
    if monitor.primary {
        content = if detail == Detail::Full {
            content.child(
                div()
                    .rounded_full()
                    .px_1p5()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(fg_hsla.opacity(0.15))
                    .child("PRIMARY"),
            )
        } else {
            content.child(div().absolute().top_0p5().right_1().text_xs().child("★"))
        };
    }

    // The wrapper's padding leaves a visible gap between touching monitors.
    div()
        .absolute()
        .p_0p5()
        .left(relative((tile.x / diagram_width) as f32))
        .top(relative((tile.y / diagram_height) as f32))
        .w(relative((tile.w / diagram_width) as f32))
        .h(relative((tile.h / diagram_height) as f32))
        .child(content)
}
