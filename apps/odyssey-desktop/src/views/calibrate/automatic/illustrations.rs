//! Small original illustrations for the two kinds of display. Inline SVG, so
//! they scale with the card, follow the light or dark theme through Tailwind
//! fill classes, and need no asset files. The shimmer and sparkle are CSS
//! animations (`input.css`) that switch off under `prefers-reduced-motion`.

use dioxus::prelude::*;

/// A friendly flat TV on a stand, with a sparkle.
#[component]
pub fn TvIllustration(class: Option<String>) -> Element {
    let class = class.unwrap_or_else(|| "h-28".into());
    rsx! {
        svg {
            class: "{class}",
            view_box: "0 0 160 120",
            fill: "none",
            "aria-hidden": "true",
            // Soft backdrop.
            ellipse { cx: "80", cy: "66", rx: "70", ry: "48", class: "fill-sky-100 dark:fill-sky-900/50" }
            // Stand.
            rect { x: "73", y: "90", width: "14", height: "11", rx: "2", class: "fill-slate-400 dark:fill-slate-500" }
            rect { x: "50", y: "100", width: "60", height: "7", rx: "3.5", class: "fill-slate-500 dark:fill-slate-400" }
            // Body and picture.
            rect { x: "20", y: "22", width: "120", height: "70", rx: "9", class: "fill-slate-700 dark:fill-slate-300" }
            rect { x: "26", y: "28", width: "108", height: "58", rx: "5", class: "fill-sky-400 dark:fill-sky-500" }
            // A glint across the picture.
            path { d: "M26 62 L72 28 H96 L26 80 Z", class: "fill-white/25" }
            // A small smile.
            circle { cx: "68", cy: "55", r: "3", class: "fill-white" }
            circle { cx: "92", cy: "55", r: "3", class: "fill-white" }
            path { d: "M68 66 Q80 76 92 66", stroke: "white", stroke_width: "3.5", stroke_linecap: "round" }
            // Sparkle.
            path {
                class: "calib-sparkle fill-amber-400 dark:fill-amber-300",
                d: "M138 6 L141 15 L150 18 L141 21 L138 30 L135 21 L126 18 L135 15 Z",
            }
            circle { cx: "22", cy: "16", r: "2.5", class: "calib-sparkle fill-sky-300 dark:fill-sky-200", style: "animation-delay: 1.1s" }
        }
    }
}

/// A projector throwing its beam onto a screen on the wall.
#[component]
pub fn ProjectorIllustration(class: Option<String>) -> Element {
    let class = class.unwrap_or_else(|| "h-28".into());
    rsx! {
        svg {
            class: "{class}",
            view_box: "0 0 160 120",
            fill: "none",
            "aria-hidden": "true",
            ellipse { cx: "80", cy: "66", rx: "70", ry: "48", class: "fill-amber-100 dark:fill-amber-900/40" }
            // The picture on the wall.
            rect { x: "98", y: "20", width: "52", height: "66", rx: "3", class: "fill-white dark:fill-slate-100" }
            rect { x: "98", y: "20", width: "52", height: "66", rx: "3", stroke_width: "2", class: "stroke-slate-300 dark:stroke-slate-400" }
            for (i, (x, y)) in [(110.0, 38.0), (124.0, 38.0), (138.0, 38.0), (110.0, 52.0), (124.0, 52.0), (138.0, 52.0), (110.0, 66.0), (124.0, 66.0), (138.0, 66.0)].into_iter().enumerate() {
                circle { key: "{i}", cx: "{x}", cy: "{y}", r: "3.2", class: "fill-slate-800" }
            }
            // The beam.
            polygon { points: "56,64 98,22 98,84", class: "calib-beam fill-amber-300/60 dark:fill-amber-200/40" }
            // Feet and body.
            rect { x: "20", y: "76", width: "6", height: "7", rx: "1.5", class: "fill-slate-500 dark:fill-slate-400" }
            rect { x: "46", y: "76", width: "6", height: "7", rx: "1.5", class: "fill-slate-500 dark:fill-slate-400" }
            rect { x: "12", y: "50", width: "48", height: "28", rx: "9", class: "fill-slate-700 dark:fill-slate-300" }
            circle { cx: "56", cy: "64", r: "9", class: "fill-slate-500 dark:fill-slate-400" }
            circle { cx: "56", cy: "64", r: "5.5", class: "fill-amber-300" }
            // A small smile.
            circle { cx: "26", cy: "60", r: "2.4", class: "fill-white dark:fill-slate-700" }
            circle { cx: "38", cy: "60", r: "2.4", class: "fill-white dark:fill-slate-700" }
            path { d: "M26 68 Q32 73 38 68", stroke_width: "2.6", stroke_linecap: "round", class: "stroke-white dark:stroke-slate-700" }
            // Sparkle.
            path {
                class: "calib-sparkle fill-sky-400 dark:fill-sky-300",
                d: "M30 8 L32.5 15 L40 17.5 L32.5 20 L30 27 L27.5 20 L20 17.5 L27.5 15 Z",
            }
        }
    }
}
