//! A "where are my screens" picker: the machine's monitors drawn to scale in
//! the arrangement the OS uses, one clickable tile per monitor.
//!
//! [`use_monitor_selection`] owns the monitor list and the remembered choice;
//! [`ScreenPicker`] draws it. Any view that needs "which monitor" shares one
//! [`MonitorSelection`] with the picker.

use std::time::Duration;

use dioxus::{
    desktop::{
        tao::{monitor::MonitorHandle, window::Fullscreen},
        use_window, Config, DesktopContext, WindowBuilder, WindowCloseBehaviour,
    },
    logger::tracing,
    prelude::*,
};
use odyssey_hub_common::config::{self, MonitorKey};

/// How often to look for monitors being plugged in or removed.
const MONITOR_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How long the numbered overlay stays on each monitor.
const IDENTIFY_DURATION: Duration = Duration::from_millis(1800);

/// Widest and tallest the diagram grows, in CSS pixels. The diagram shrinks
/// below the width when its container is narrower.
const DIAGRAM_MAX_WIDTH: f64 = 416.0;
const DIAGRAM_MAX_HEIGHT: f64 = 200.0;

/* ------------------------------ Pure logic -------------------------------- */

/// What the picker knows about one monitor.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    pub key: MonitorKey,
    pub primary: bool,
}

impl MonitorInfo {
    fn from_handle(handle: &MonitorHandle, primary: Option<&MonitorKey>) -> Self {
        let key = monitor_key(handle);
        let primary = primary == Some(&key);
        Self { key, primary }
    }

    pub fn display_name(&self) -> &str {
        self.key.name.as_deref().unwrap_or("Unknown display")
    }

    pub fn resolution(&self) -> String {
        format!("{} × {}", self.key.width, self.key.height)
    }
}

pub(crate) fn monitor_key(handle: &MonitorHandle) -> MonitorKey {
    let position = handle.position();
    let size = handle.size();
    MonitorKey {
        name: handle.name(),
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    }
}

/// Index of the monitor to show as selected: the remembered one if it is still
/// attached, otherwise the primary monitor, otherwise the first.
fn resolve_selected(monitors: &[MonitorInfo], remembered: Option<&MonitorKey>) -> Option<usize> {
    remembered
        .and_then(|key| monitors.iter().position(|m| &m.key == key))
        .or_else(|| monitors.iter().position(|m| m.primary))
        .or(if monitors.is_empty() { None } else { Some(0) })
}

/// A monitor's rectangle in the diagram, in diagram pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tile {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct Layout {
    width: f64,
    height: f64,
    tiles: Vec<Tile>,
}

/// Scales the monitors' physical-pixel rectangles to fit `max_w` × `max_h`,
/// preserving the arrangement. Positions may be negative (a monitor left of or
/// above the primary), so the layout is relative to the bounding box's corner.
/// The scale never exceeds 1 so a tiny virtual desktop is not blown up.
fn layout(monitors: &[MonitorInfo], max_w: f64, max_h: f64) -> Option<Layout> {
    if monitors.is_empty() {
        return None;
    }

    // A zero-sized monitor would collapse its tile and could zero the bounding
    // box, so every extent is at least one pixel.
    let rects: Vec<(i64, i64, i64, i64)> = monitors
        .iter()
        .map(|m| {
            (
                i64::from(m.key.x),
                i64::from(m.key.y),
                i64::from(m.key.width.max(1)),
                i64::from(m.key.height.max(1)),
            )
        })
        .collect();

    let min_x = rects.iter().map(|r| r.0).min()?;
    let min_y = rects.iter().map(|r| r.1).min()?;
    let max_x = rects.iter().map(|r| r.0 + r.2).max()?;
    let max_y = rects.iter().map(|r| r.1 + r.3).max()?;
    let span_w = (max_x - min_x) as f64;
    let span_h = (max_y - min_y) as f64;

    let scale = (max_w / span_w).min(max_h / span_h).min(1.0);
    Some(Layout {
        width: span_w * scale,
        height: span_h * scale,
        tiles: rects
            .iter()
            .map(|&(x, y, w, h)| Tile {
                x: (x - min_x) as f64 * scale,
                y: (y - min_y) as f64 * scale,
                w: w as f64 * scale,
                h: h as f64 * scale,
            })
            .collect(),
    })
}

/// How much a tile can say about its monitor before the text overflows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Detail {
    /// Number only.
    Minimal,
    /// Number and resolution.
    Compact,
    /// Number, name and resolution.
    Named,
    /// Number, name, resolution and the primary tag.
    Full,
}

/// The height thresholds add up the stacked text rows (badge 24 px, name
/// 16 px, resolution 14 px, tag 16 px) plus the tile's border and padding.
fn detail_for(tile: &Tile) -> Detail {
    if tile.w >= 120.0 && tile.h >= 92.0 {
        Detail::Full
    } else if tile.w >= 110.0 && tile.h >= 72.0 {
        Detail::Named
    } else if tile.w >= 72.0 && tile.h >= 52.0 {
        Detail::Compact
    } else {
        Detail::Minimal
    }
}

/* ------------------------------ Selection state --------------------------- */

fn enumerate_monitors(window: &DesktopContext) -> Vec<MonitorInfo> {
    let primary = window.primary_monitor().map(|m| monitor_key(&m));
    window
        .available_monitors()
        .map(|m| MonitorInfo::from_handle(&m, primary.as_ref()))
        .collect()
}

/// The attached monitors and the one the user picked. Copy it freely: it is a
/// pair of signal handles.
#[derive(Clone, Copy, PartialEq)]
pub struct MonitorSelection {
    monitors: Signal<Vec<MonitorInfo>>,
    chosen: Signal<Option<MonitorKey>>,
}

impl MonitorSelection {
    pub fn monitors(&self) -> Vec<MonitorInfo> {
        self.monitors.read().clone()
    }

    /// Index into [`Self::monitors`] of the selected monitor, if any are attached.
    pub fn selected(&self) -> Option<usize> {
        resolve_selected(&self.monitors.read(), self.chosen.read().as_ref())
    }

    /// The OS handle of the selected monitor, for placing windows on it.
    pub fn selected_handle(&self) -> Option<MonitorHandle> {
        let key = self.monitors.read().get(self.selected()?)?.key.clone();
        dioxus::desktop::window()
            .available_monitors()
            .find(|m| monitor_key(m) == key)
    }

    /// Selects the monitor at `index` and remembers it across launches.
    pub fn select(&self, index: usize) {
        let Some(key) = self.monitors.peek().get(index).map(|m| m.key.clone()) else {
            return;
        };
        let mut chosen = self.chosen;
        chosen.set(Some(key.clone()));

        spawn(async move {
            // Reload rather than reuse an earlier snapshot so a field some
            // other part of the app saved in the meantime is not overwritten.
            let mut state = config::ui_state_async().await.into_value();
            state.selected_monitor = Some(key);
            if let Err(e) = config::ui_state_save_async(&state).await {
                tracing::warn!("Failed to save UI state: {e}");
            }
        });
    }
}

/// Tracks the attached monitors (including hot-plug) and restores the
/// monitor the user picked last time.
pub fn use_monitor_selection() -> MonitorSelection {
    let window = use_window();
    let mut monitors = use_signal({
        let window = window.clone();
        move || enumerate_monitors(&window)
    });
    let mut chosen = use_signal(|| None::<MonitorKey>);

    use_future(move || async move {
        let remembered = config::ui_state_async().await.into_value().selected_monitor;
        // A click that lands before the file loads wins over the stale value.
        if chosen.peek().is_none() {
            chosen.set(remembered);
        }
    });

    use_future(move || {
        let window = window.clone();
        async move {
            loop {
                tokio::time::sleep(MONITOR_POLL_INTERVAL).await;
                let latest = enumerate_monitors(&window);
                if *monitors.peek() != latest {
                    monitors.set(latest);
                }
            }
        }
    });

    MonitorSelection { monitors, chosen }
}

/* -------------------------------- Identify -------------------------------- */

/// Puts a numbered overlay on every physical monitor for a moment. The numbers
/// match the ones on the picker's tiles because both follow the OS's order.
fn identify_monitors(window: &DesktopContext) {
    let primary = window.primary_monitor().map(|m| monitor_key(&m));
    for (i, monitor) in window.available_monitors().enumerate() {
        let info = MonitorInfo::from_handle(&monitor, primary.as_ref());
        let props = IdentifyOverlayProps {
            number: i + 1,
            label: format!("{} · {}", info.display_name(), info.resolution()),
        };
        let dom = VirtualDom::new_with_props(IdentifyOverlay, props);
        let config = Config::default()
            .with_menu(None)
            .with_close_behaviour(WindowCloseBehaviour::WindowCloses)
            .with_window(
                WindowBuilder::new()
                    .with_position(monitor.position())
                    .with_fullscreen(Some(Fullscreen::Borderless(Some(monitor)))),
            );
        window.new_window(dom, config);
    }
}

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
fn IdentifyOverlay(number: usize, label: String) -> Element {
    use_future(|| async {
        tokio::time::sleep(IDENTIFY_DURATION).await;
        dioxus::desktop::window().close();
    });

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }

        div {
            class: "h-screen flex flex-col items-center justify-center gap-6 bg-blue-700 text-white select-none cursor-pointer",
            onclick: move |_| dioxus::desktop::window().close(),
            div { class: "text-[16rem] leading-none font-bold", "{number}" }
            div { class: "text-2xl opacity-90", "{label}" }
        }
    }
}

/* --------------------------------- Picker --------------------------------- */

const LABEL_ID: &str = "screen-picker-label";

#[component]
pub fn ScreenPicker(selection: MonitorSelection) -> Element {
    let window = use_window();
    let mut identifying = use_signal(|| false);

    let monitors = selection.monitors();
    let selected = selection.selected();
    let layout = layout(&monitors, DIAGRAM_MAX_WIDTH, DIAGRAM_MAX_HEIGHT);

    rsx! {
        div {
            class: "flex flex-col gap-3",

            div {
                class: "flex flex-row items-center justify-between",
                span {
                    id: LABEL_ID,
                    class: "text-base font-medium text-gray-500 dark:text-gray-400",
                    "Choose a display:"
                }
                button {
                    class: "btn-secondary",
                    r#type: "button",
                    disabled: monitors.is_empty() || identifying(),
                    title: "Briefly show each display's number on that display",
                    onclick: move |_| {
                        let window = window.clone();
                        async move {
                            identifying.set(true);
                            identify_monitors(&window);
                            tokio::time::sleep(IDENTIFY_DURATION + Duration::from_millis(400)).await;
                            identifying.set(false);
                        }
                    },
                    "Identify"
                }
            }

            if let Some(layout) = layout {
                div {
                    class: "flex justify-center rounded-lg bg-gray-100 dark:bg-gray-800 p-3",
                    div {
                        role: "radiogroup",
                        aria_labelledby: LABEL_ID,
                        class: "relative",
                        style: "width: min(100%, {layout.width:.1}px); aspect-ratio: {layout.width:.3} / {layout.height:.3};",
                        for (i, (monitor, tile)) in monitors.iter().zip(layout.tiles.iter()).enumerate() {
                            MonitorTile {
                                key: "{i}:{monitor.key.x},{monitor.key.y}:{monitor.key.width}x{monitor.key.height}",
                                number: i + 1,
                                monitor: monitor.clone(),
                                left: tile.x / layout.width * 100.0,
                                top: tile.y / layout.height * 100.0,
                                width: tile.w / layout.width * 100.0,
                                height: tile.h / layout.height * 100.0,
                                detail: detail_for(tile),
                                selected: selected == Some(i),
                                onselect: move |_| selection.select(i),
                            }
                        }
                    }
                }

                if let Some((i, monitor)) = selected.and_then(|i| monitors.get(i).map(|m| (i, m))) {
                    div {
                        class: "text-sm text-gray-600 dark:text-gray-300 text-center",
                        "{i + 1}. {monitor.display_name()} — {monitor.resolution()}"
                        if monitor.primary {
                            " — Primary"
                        }
                    }
                }
                if monitors.iter().zip(layout.tiles.iter()).any(|(m, t)| m.primary && detail_for(t) != Detail::Full) {
                    div {
                        class: "text-xs text-gray-500 dark:text-gray-400 text-center",
                        "★ primary display"
                    }
                }
            } else {
                div {
                    class: "rounded-lg bg-gray-100 dark:bg-gray-800 p-6 text-center text-sm text-gray-600 dark:text-gray-300",
                    "No displays found."
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct MonitorTileProps {
    number: usize,
    monitor: MonitorInfo,
    /// Percent of the diagram.
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    detail: Detail,
    selected: bool,
    onselect: EventHandler<()>,
}

#[component]
fn MonitorTile(props: MonitorTileProps) -> Element {
    let MonitorTileProps {
        number,
        monitor,
        left,
        top,
        width,
        height,
        detail,
        selected,
        onselect,
    } = props;

    let tile_class = if selected {
        "bg-blue-600 border-blue-300 text-white shadow-md hover:bg-blue-500 dark:border-blue-400"
    } else {
        "bg-gray-200 border-gray-300 text-gray-700 hover:bg-gray-300 hover:border-blue-400 dark:bg-gray-600 dark:border-gray-500 dark:text-gray-100 dark:hover:bg-gray-500 dark:hover:border-blue-400"
    };
    let badge_class = if selected {
        "bg-white text-blue-700"
    } else {
        "bg-white/70 text-gray-900 dark:bg-black/30 dark:text-white"
    };
    let description = format!(
        "{number}. {}, {}{}",
        monitor.display_name(),
        monitor.resolution(),
        if monitor.primary { ", primary" } else { "" }
    );

    rsx! {
        // The wrapper's padding leaves a visible gap between touching monitors.
        div {
            class: "absolute p-0.5",
            style: "left: {left:.3}%; top: {top:.3}%; width: {width:.3}%; height: {height:.3}%;",
            button {
                class: "relative w-full h-full overflow-hidden rounded-md border-2 px-1 flex flex-col items-center justify-center gap-0.5 text-center cursor-pointer transition-colors focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:focus-visible:ring-blue-800 {tile_class}",
                r#type: "button",
                role: "radio",
                aria_checked: selected,
                aria_label: "{description}",
                title: "{description}",
                onclick: move |_| onselect.call(()),

                span {
                    class: "flex items-center justify-center size-6 shrink-0 rounded-full text-sm font-bold {badge_class}",
                    "{number}"
                }
                if matches!(detail, Detail::Named | Detail::Full) {
                    span { class: "max-w-full truncate text-xs font-medium", "{monitor.display_name()}" }
                }
                if detail != Detail::Minimal {
                    span { class: "max-w-full truncate text-[11px] opacity-80", "{monitor.resolution()}" }
                }
                if monitor.primary {
                    if detail == Detail::Full {
                        span {
                            class: "rounded-full px-1.5 text-[10px] font-semibold uppercase tracking-wide bg-black/15 dark:bg-white/20",
                            "Primary"
                        }
                    } else {
                        span { class: "absolute top-0.5 right-1 text-xs", aria_hidden: true, "★" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(
        name: Option<&str>,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        primary: bool,
    ) -> MonitorInfo {
        MonitorInfo {
            key: MonitorKey {
                name: name.map(str::to_string),
                x,
                y,
                width,
                height,
            },
            primary,
        }
    }

    fn assert_close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn identical_monitors_get_distinct_keys_by_position() {
        let left = monitor(Some("DELL U2723QE"), 0, 0, 2560, 1440, true);
        let right = monitor(Some("DELL U2723QE"), 2560, 0, 2560, 1440, false);
        assert_ne!(left.key, right.key);

        let monitors = [left, right.clone()];
        assert_eq!(resolve_selected(&monitors, Some(&right.key)), Some(1));
    }

    #[test]
    fn remembered_monitor_wins_over_primary() {
        let monitors = [
            monitor(Some("A"), 0, 0, 1920, 1080, true),
            monitor(Some("B"), 1920, 0, 1920, 1080, false),
        ];
        assert_eq!(resolve_selected(&monitors, Some(&monitors[1].key)), Some(1));
    }

    #[test]
    fn missing_remembered_monitor_falls_back_to_primary() {
        let monitors = [
            monitor(Some("A"), -1920, 0, 1920, 1080, false),
            monitor(Some("B"), 0, 0, 1920, 1080, true),
        ];
        let gone = monitor(Some("C"), 1920, 0, 1920, 1080, false).key;
        assert_eq!(resolve_selected(&monitors, Some(&gone)), Some(1));
        assert_eq!(resolve_selected(&monitors, None), Some(1));
    }

    #[test]
    fn a_moved_monitor_is_not_the_remembered_one() {
        let monitors = [
            monitor(Some("A"), 0, 0, 1920, 1080, true),
            monitor(Some("B"), 0, -1080, 1920, 1080, false),
        ];
        let before_rearranging = monitor(Some("B"), 1920, 0, 1920, 1080, false).key;
        assert_eq!(
            resolve_selected(&monitors, Some(&before_rearranging)),
            Some(0)
        );
    }

    #[test]
    fn without_a_primary_the_first_monitor_is_selected() {
        let monitors = [
            monitor(None, 0, 0, 1920, 1080, false),
            monitor(None, 1920, 0, 1920, 1080, false),
        ];
        assert_eq!(resolve_selected(&monitors, None), Some(0));
        assert_eq!(resolve_selected(&[], None), None);
    }

    #[test]
    fn layout_fits_width_and_preserves_arrangement() {
        // Two 1920×1080 side by side: 3840×1080, width-limited.
        let monitors = [
            monitor(None, 0, 0, 1920, 1080, true),
            monitor(None, 1920, 0, 1920, 1080, false),
        ];
        let l = layout(&monitors, 384.0, 200.0).unwrap();
        assert_close(l.width, 384.0);
        assert_close(l.height, 108.0);
        assert_eq!(
            l.tiles[0],
            Tile {
                x: 0.0,
                y: 0.0,
                w: 192.0,
                h: 108.0
            }
        );
        assert_eq!(
            l.tiles[1],
            Tile {
                x: 192.0,
                y: 0.0,
                w: 192.0,
                h: 108.0
            }
        );
    }

    #[test]
    fn layout_fits_height_for_stacked_monitors() {
        // 1080×1920 portrait on top of another: 1080×3840, height-limited.
        let monitors = [
            monitor(None, 0, 0, 1080, 1920, true),
            monitor(None, 0, 1920, 1080, 1920, false),
        ];
        let l = layout(&monitors, 400.0, 192.0).unwrap();
        assert_close(l.height, 192.0);
        assert_close(l.width, 54.0);
        assert_close(l.tiles[1].y, 96.0);
    }

    #[test]
    fn layout_handles_negative_and_offset_positions() {
        // A monitor left of and above the primary, and a taller one offset down.
        let monitors = [
            monitor(None, -1000, -500, 1000, 500, false),
            monitor(None, 0, 0, 1000, 1000, true),
            monitor(None, 1000, 250, 1000, 1000, false),
        ];
        let l = layout(&monitors, 1.0e6, 1.0e6).unwrap();
        // The scale is capped at 1, so tiles are in physical pixels relative
        // to the bounding box's top-left corner at (-1000, -500).
        assert_close(l.width, 3000.0);
        assert_close(l.height, 1750.0);
        assert_eq!(
            l.tiles[0],
            Tile {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 500.0
            }
        );
        assert_eq!(
            l.tiles[1],
            Tile {
                x: 1000.0,
                y: 500.0,
                w: 1000.0,
                h: 1000.0
            }
        );
        assert_eq!(
            l.tiles[2],
            Tile {
                x: 2000.0,
                y: 750.0,
                w: 1000.0,
                h: 1000.0
            }
        );
    }

    #[test]
    fn layout_never_upscales_and_tolerates_empty_or_degenerate_monitors() {
        assert_eq!(layout(&[], 400.0, 200.0), None);

        let tiny = [monitor(None, 0, 0, 100, 50, true)];
        let l = layout(&tiny, 400.0, 200.0).unwrap();
        assert_close(l.width, 100.0);

        let zero = [monitor(None, 0, 0, 0, 0, true)];
        let l = layout(&zero, 400.0, 200.0).unwrap();
        assert!(l.width > 0.0 && l.height > 0.0);
    }

    #[test]
    fn detail_shrinks_with_tile_size() {
        let tile = |w, h| Tile {
            x: 0.0,
            y: 0.0,
            w,
            h,
        };
        assert_eq!(detail_for(&tile(356.0, 200.0)), Detail::Full);
        assert_eq!(detail_for(&tile(130.0, 80.0)), Detail::Named);
        assert_eq!(detail_for(&tile(100.0, 56.0)), Detail::Compact);
        assert_eq!(detail_for(&tile(60.0, 34.0)), Detail::Minimal);
        // A wide strip too short for any text beside the number.
        assert_eq!(detail_for(&tile(300.0, 30.0)), Detail::Minimal);
    }
}
