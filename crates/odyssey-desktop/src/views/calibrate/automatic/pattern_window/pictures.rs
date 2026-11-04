//! What the pattern window draws in each mode.
//!
//! Every picture is an SVG whose viewBox is the monitor's own pixels, so a
//! unit is one pixel and the ruler and the card are as long as the tech is
//! told they are.

use dioxus::prelude::*;

use super::display::Link;
use crate::views::calibrate::automatic::reference;

/// The gray of the instructions under the ruler and the card.
const HINT_COLOR: &str = "#9ca3af";
const HINT_FONT_SIZE: &str = "28";

/// Half the height of the tick at each end of the ruler's line, in pixels.
const RULER_TICK_HALF_HEIGHT: f64 = 40.0;
/// Gap from the ruler's line to its instructions, and to the second line.
const RULER_HINT_OFFSET: f64 = 96.0;
const RULER_BACK_HINT_OFFSET: f64 = 144.0;
/// Thickness of the white frame around the card's inside, in pixels.
const CARD_FRAME: f64 = 4.0;
/// Gap from the card's frame to its instructions, and to the second line.
const CARD_HINT_OFFSET: f64 = 64.0;
const CARD_BACK_HINT_OFFSET: f64 = 112.0;

/// One line of instructions, centered on `x`.
#[component]
fn Hint(x: f64, y: f64, text: &'static str) -> Element {
    rsx! {
        text {
            x: "{x}",
            y: "{y}",
            text_anchor: "middle",
            fill: HINT_COLOR,
            font_size: HINT_FONT_SIZE,
            font_family: "sans-serif",
            {text}
        }
    }
}

#[component]
pub(super) fn PatternPicture(link: Link, screen_w: f64, screen_h: f64) -> Element {
    let pattern = &link.0.pattern;
    // The viewBox is the monitor's own pixels, as for the ruler and the
    // card. Dots stay round because the pattern's aspect is the monitor's.
    //
    // FIXME(webkit): with the viewBox in the pattern's own unit space (about
    // 1.78 x 1), WebKit does not paint the right-hand ring (left, top and
    // bottom are fine), so a display showing its whole picture reads as
    // cropped on the right. A bare WebKitGTK page with the same SVG does the
    // same, so it is not this app's layout or window; resvg paints the ring.
    // At 1920 x 1080 a 1.78 x 1 viewBox loses it, while 2 x 1, 17.8 x 10,
    // 178 x 100, 1778 x 1000 and 1920 x 1080 paint it. Cause unknown; pixel
    // units work and match the ruler and card. Worth a look in WebKit if it
    // shows up elsewhere.
    let (sx, sy) = (screen_w / pattern.aspect(), screen_h);
    rsx! {
        svg {
            class: "size-full",
            view_box: "0 0 {screen_w} {screen_h}",
            preserve_aspect_ratio: "none",
            shape_rendering: "geometricPrecision",
            // The border ring and corner brackets, whose visibility
            // tells the camera whether the display crops the picture.
            for (i, rect) in pattern.frame_rects().iter().enumerate() {
                rect {
                    key: "frame-{i}",
                    x: "{rect.x * sx}",
                    y: "{rect.y * sy}",
                    width: "{rect.width * sx}",
                    height: "{rect.height * sy}",
                    fill: "white",
                }
            }
            for (i, dot) in pattern.dots().iter().enumerate() {
                circle {
                    key: "{i}",
                    cx: "{dot.u * screen_w}",
                    cy: "{dot.v * sy}",
                    r: "{dot.radius_h * sy}",
                    fill: "white",
                }
            }
        }
    }
}

#[component]
pub(super) fn RulerPicture(
    line_px: u32,
    covers_wizard: bool,
    screen_w: f64,
    screen_h: f64,
) -> Element {
    let line = f64::from(line_px);
    let x0 = ((screen_w - line) / 2.0).floor();
    let y = (screen_h / 2.0).floor();
    let tick_top = y - RULER_TICK_HALF_HEIGHT;
    let tick_height = 2.0 * RULER_TICK_HALF_HEIGHT;
    rsx! {
        svg {
            class: "size-full",
            view_box: "0 0 {screen_w} {screen_h}",
            preserve_aspect_ratio: "none",
            shape_rendering: "crispEdges",
            rect { x: "{x0}", y: "{y - 1.0}", width: "{line}", height: "2", fill: "white" }
            rect { x: "{x0 - 1.0}", y: "{tick_top}", width: "2", height: "{tick_height}", fill: "white" }
            rect { x: "{x0 + line - 1.0}", y: "{tick_top}", width: "2", height: "{tick_height}", fill: "white" }
            Hint {
                x: screen_w / 2.0,
                y: y + RULER_HINT_OFFSET,
                text: "Measure from the middle of one tick to the middle of the other",
            }
            if covers_wizard {
                Hint {
                    x: screen_w / 2.0,
                    y: y + RULER_BACK_HINT_OFFSET,
                    text: "Press Escape or Enter to come back and type what you read",
                }
            }
        }
    }
}

#[component]
pub(super) fn CardPicture(
    width_px: f64,
    covers_wizard: bool,
    screen_w: f64,
    screen_h: f64,
) -> Element {
    let height_px = width_px * reference::CARD_HEIGHT_MM / reference::CARD_WIDTH_MM;
    let radius = width_px * reference::CARD_CORNER_OF_WIDTH;
    let (left, top) = ((screen_w - width_px) / 2.0, (screen_h - height_px) / 2.0);
    let hint_base = top + height_px + CARD_FRAME;
    rsx! {
        svg {
            class: "size-full",
            view_box: "0 0 {screen_w} {screen_h}",
            preserve_aspect_ratio: "none",
            shape_rendering: "geometricPrecision",
            // A white frame whose inside edge is exactly the card: a card of
            // the right size fills it with no dark gap and covers none of it.
            rect {
                x: "{left - CARD_FRAME}",
                y: "{top - CARD_FRAME}",
                width: "{width_px + 2.0 * CARD_FRAME}",
                height: "{height_px + 2.0 * CARD_FRAME}",
                rx: "{radius + CARD_FRAME}",
                fill: "white",
            }
            rect {
                x: "{left}",
                y: "{top}",
                width: "{width_px}",
                height: "{height_px}",
                rx: "{radius}",
                fill: "black",
            }
            Hint {
                x: screen_w / 2.0,
                y: hint_base + CARD_HINT_OFFSET,
                text: "Hold a card flat against the glass, inside the frame",
            }
            if covers_wizard {
                Hint {
                    x: screen_w / 2.0,
                    y: hint_base + CARD_BACK_HINT_OFFSET,
                    text: "Arrow keys resize the frame (Page Up and Page Down in bigger steps). Press Enter when it fits.",
                }
            }
        }
    }
}

#[component]
pub(super) fn Countdown(seconds: u32) -> Element {
    rsx! {
        div {
            class: "size-full flex flex-col items-center justify-center gap-6 text-white",
            div { class: "text-3xl opacity-80", "Measuring in" }
            div { class: "text-[14rem] leading-none font-bold tabular-nums", "{seconds}" }
            div { class: "text-2xl opacity-80", "Hold the camera steady" }
        }
    }
}
