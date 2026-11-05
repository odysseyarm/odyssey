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
                    class: "text-base font-medium text-fg-muted",
                    "Choose a display:"
                }
                button {
                    class: "btn btn-secondary btn-sm",
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
                    class: "flex justify-center rounded-lg bg-surface-alt p-3",
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
                        class: "text-sm text-fg-muted text-center",
                        "{i + 1}. {monitor.display_name()} — {monitor.resolution()}"
                        if monitor.primary {
                            " — Primary"
                        }
                    }
                }
                if monitors.iter().zip(layout.tiles.iter()).any(|(m, t)| m.primary && detail_for(t) != Detail::Full) {
                    div {
                        class: "text-xs text-fg-muted text-center",
                        "★ primary display"
                    }
                }
            } else {
                div {
                    class: "rounded-lg bg-surface-alt p-6 text-center text-sm text-fg-muted",
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
        "bg-accent border-accent-strong text-on-accent shadow-md hover:bg-accent-strong"
    } else {
        "bg-surface border-line-strong text-fg hover:bg-surface-hover hover:border-accent-ink"
    };
    let badge_class = if selected {
        "bg-black/15 text-on-accent"
    } else {
        "bg-surface-alt text-fg-strong"
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
                class: "relative w-full h-full overflow-hidden rounded-lg border-2 px-1 flex flex-col items-center justify-center gap-0.5 text-center cursor-pointer transition-colors {tile_class}",
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
                            class: "rounded-full px-1.5 text-[10px] font-semibold uppercase tracking-wide bg-current/15",
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
