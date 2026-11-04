use std::rc::Rc;
use std::time::Duration;

use dioxus::desktop::use_window;
use dioxus::prelude::*;

use super::identify_overlay::{IDENTIFY_DURATION, identify_monitors};
use super::monitor_layout::{Detail, MonitorInfo, detail_for, layout};
use super::monitor_selection::MonitorSelection;

/// Widest and tallest the diagram grows, in CSS pixels. The diagram shrinks
/// below the width when its container is narrower.
const DIAGRAM_MAX_WIDTH: f64 = 416.0;
const DIAGRAM_MAX_HEIGHT: f64 = 200.0;

const LABEL_ID: &str = "screen-picker-label";

#[component]
pub(crate) fn ScreenPicker(selection: MonitorSelection) -> Element {
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
                        let window = Rc::clone(&window);
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
                                onselect: move |()| selection.select(i),
                            }
                        }
                    }
                }

                // A tile too small for its name and resolution gets them spelled out here.
                if let Some((i, monitor)) = selected
                    .filter(|&i| layout.tiles.get(i).is_some_and(|tile| detail_for(tile) != Detail::Full))
                    .and_then(|i| monitors.get(i).map(|m| (i, m)))
                {
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
                aria_label: description.clone(),
                title: description,
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
