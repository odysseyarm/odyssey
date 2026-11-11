//! What the pattern window draws in each mode.
//!
//! Every picture is laid out in the monitor's own pixel units and scaled to the
//! window, so the ruler and the card are as long as the tech is told they are.

use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Bounds, Corners, IntoElement as _, ParentElement as _, Pixels, Rgba, Styled as _,
    canvas, div, fill, point, px, size, white,
};
use odyssey_autocalib::pattern::Pattern;

use crate::views::calibrate::automatic::reference;

const HINT_FONT_SIZE: f32 = 28.0;

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

/// Maps the monitor's pixel units onto the window.
#[derive(Clone, Copy)]
pub(super) struct Scale {
    /// The monitor's size in its own pixels.
    pub width: f64,
    pub height: f64,
    /// Window pixels per monitor pixel.
    pub x: f32,
    pub y: f32,
}

impl Scale {
    fn px_x(self, value: f64) -> Pixels {
        px(value as f32 * self.x)
    }

    fn px_y(self, value: f64) -> Pixels {
        px(value as f32 * self.y)
    }

    /// A white rectangle at monitor-pixel coordinates.
    fn rect(self, x: f64, y: f64, width: f64, height: f64) -> gpui::Div {
        div()
            .absolute()
            .left(self.px_x(x))
            .top(self.px_y(y))
            .w(self.px_x(width))
            .h(self.px_y(height))
            .bg(white())
    }

    /// One line of instructions, centered on the screen.
    fn hint(self, y: f64, text: &'static str, color: Rgba) -> gpui::Div {
        div()
            .absolute()
            .left(px(0.0))
            .w(self.px_x(self.width))
            .top(self.px_y(y - f64::from(HINT_FONT_SIZE)))
            .flex()
            .justify_center()
            .text_size(px(HINT_FONT_SIZE * self.y))
            .text_color(color)
            .child(text)
    }
}

/// The border ring and corner brackets, whose visibility tells the camera
/// whether the display crops the picture, and the dots. The dots stay round
/// because the pattern's aspect is the monitor's.
pub(super) fn pattern_picture(pattern: Rc<Pattern>) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, (), window, _| {
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let sx = w / pattern.aspect() as f32;
            let white = white();
            for rect in pattern.frame_rects() {
                let origin = bounds.origin + point(px(rect.x as f32 * sx), px(rect.y as f32 * h));
                let extent = size(px(rect.width as f32 * sx), px(rect.height as f32 * h));
                window.paint_quad(fill(Bounds::new(origin, extent), white));
            }
            for dot in pattern.dots() {
                let radius = dot.radius_h as f32 * h;
                let center = bounds.origin + point(px(dot.u as f32 * w), px(dot.v as f32 * h));
                let corner = point(center.x - px(radius), center.y - px(radius));
                window.paint_quad(
                    fill(
                        Bounds::new(corner, size(px(2.0 * radius), px(2.0 * radius))),
                        white,
                    )
                    .corner_radii(Corners::all(px(radius))),
                );
            }
        },
    )
    .size_full()
    .into_any_element()
}

pub(super) fn ruler_picture(
    line_px: u32,
    covers_wizard: bool,
    scale: Scale,
    hint_color: Rgba,
) -> AnyElement {
    let line = f64::from(line_px);
    let x0 = ((scale.width - line) / 2.0).floor();
    let y = (scale.height / 2.0).floor();
    let tick_top = y - RULER_TICK_HALF_HEIGHT;
    let tick_height = 2.0 * RULER_TICK_HALF_HEIGHT;
    div()
        .size_full()
        .relative()
        .child(scale.rect(x0, y - 1.0, line, 2.0))
        .child(scale.rect(x0 - 1.0, tick_top, 2.0, tick_height))
        .child(scale.rect(x0 + line - 1.0, tick_top, 2.0, tick_height))
        .child(scale.hint(
            y + RULER_HINT_OFFSET,
            "Measure from the middle of one tick to the middle of the other",
            hint_color,
        ))
        .when(covers_wizard, |picture| {
            picture.child(scale.hint(
                y + RULER_BACK_HINT_OFFSET,
                "Press Escape or Enter to come back and type what you read",
                hint_color,
            ))
        })
        .into_any_element()
}

pub(super) fn card_picture(
    width_px: f64,
    covers_wizard: bool,
    scale: Scale,
    hint_color: Rgba,
) -> AnyElement {
    let height_px = width_px * reference::CARD_HEIGHT_MM / reference::CARD_WIDTH_MM;
    let radius = width_px * reference::CARD_CORNER_OF_WIDTH;
    let (left, top) = (
        (scale.width - width_px) / 2.0,
        (scale.height - height_px) / 2.0,
    );
    let hint_base = top + height_px + CARD_FRAME;
    // A white frame whose inside edge is exactly the card: a card of the right
    // size fills it with no dark gap and covers none of it.
    let frame = scale
        .rect(
            left - CARD_FRAME,
            top - CARD_FRAME,
            width_px + 2.0 * CARD_FRAME,
            height_px + 2.0 * CARD_FRAME,
        )
        .rounded(scale.px_x(radius + CARD_FRAME));
    let inside = scale
        .rect(left, top, width_px, height_px)
        .bg(gpui::black())
        .rounded(scale.px_x(radius));
    div()
        .size_full()
        .relative()
        .child(frame)
        .child(inside)
        .child(scale.hint(
            hint_base + CARD_HINT_OFFSET,
            "Hold a card flat against the glass, inside the frame",
            hint_color,
        ))
        .when(covers_wizard, |picture| {
            picture.child(scale.hint(
                hint_base + CARD_BACK_HINT_OFFSET,
                "Arrow keys resize the frame (Page Up and Page Down in bigger steps). Press Enter when it fits.",
                hint_color,
            ))
        })
        .into_any_element()
}

pub(super) fn countdown(seconds: u32) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_6()
        .text_color(white())
        .child(div().text_size(px(30.0)).opacity(0.8).child("Measuring in"))
        .child(
            div()
                .text_size(px(224.0))
                .line_height(px(224.0))
                .font_weight(gpui::FontWeight::BOLD)
                .child(seconds.to_string()),
        )
        .child(
            div()
                .text_size(px(24.0))
                .opacity(0.8)
                .child("Hold the camera steady"),
        )
        .into_any_element()
}
