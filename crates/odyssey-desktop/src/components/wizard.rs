//! Building blocks for step-by-step flows: a stepper, a big status banner,
//! labelled sliders and a live image with an overlay. They hold no flow logic,
//! so any wizard can reuse them; the automatic calibration is the one that
//! does.

use std::sync::Arc;
use std::time::Duration;

use gpui::component::slider::{Slider as SliderWidget, SliderState};
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, BorderStyle, Bounds, Div, Entity, FontWeight,
    IntoElement, ObjectFit, ParentElement as _, PathBuilder, Pixels, RenderImage, Rgba,
    SharedString, Styled as _, StyledImage as _, Svg, Transformation, Window, canvas, div, img,
    percentage, px, relative, svg,
};

use crate::theme::{Palette, RADIUS_LG};

macro_rules! icons {
    ($($name:ident => $path:literal,)*) => {
        /// Shape of an icon, drawn on a 24-unit grid with a 1.5 stroke.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum Icon {
            $($name),*
        }

        impl Icon {
            fn svg_data(self) -> &'static [u8] {
                match self {
                    $(Self::$name => concat!(
                        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" "#,
                        r#"stroke="black" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">"#,
                        r#"<path d=""#, $path, r#""/></svg>"#
                    ).as_bytes()),*
                }
            }
        }
    };
}

icons! {
    Check => "M9 12.75L11.25 15 15 9.75M21 12a9 9 0 11-18 0 9 9 0 0118 0z",
    Camera => "M6.827 6.175A2.31 2.31 0 015.186 7.23c-.38.054-.757.112-1.134.175C2.999 7.58 2.25 8.507 2.25 9.574V18a2.25 2.25 0 002.25 2.25h15A2.25 2.25 0 0021.75 18V9.574c0-1.067-.75-1.994-1.802-2.169a47.865 47.865 0 00-1.134-.175 2.31 2.31 0 01-1.64-1.055l-.822-1.316a2.192 2.192 0 00-1.736-1.039 48.774 48.774 0 00-5.232 0 2.192 2.192 0 00-1.736 1.039l-.821 1.316zM16.5 12.75a4.5 4.5 0 11-9 0 4.5 4.5 0 019 0z",
    Viewfinder => "M7.5 3.75H6A2.25 2.25 0 003.75 6v1.5M16.5 3.75H18A2.25 2.25 0 0120.25 6v1.5m0 9V18A2.25 2.25 0 0118 20.25h-1.5m-9 0H6A2.25 2.25 0 013.75 18v-1.5M15 12a3 3 0 11-6 0 3 3 0 016 0z",
    Sun => "M12 3v2.25m6.364.386l-1.591 1.591M21 12h-2.25m-.386 6.364l-1.591-1.591M12 18.75V21m-4.773-4.227l-1.591 1.591M5.25 12H3m4.227-4.773L5.636 5.636M15.75 12a3.75 3.75 0 11-7.5 0 3.75 3.75 0 017.5 0z",
    Moon => "M21.752 15.002A9.718 9.718 0 0118 15.75c-5.385 0-9.75-4.365-9.75-9.75 0-1.33.266-2.597.748-3.752A9.753 9.753 0 003 11.25C3 16.635 7.365 21 12.75 21a9.753 9.753 0 009.002-5.998z",
    MoveAway => "M3.75 3.75v4.5m0-4.5h4.5m-4.5 0L9 9M3.75 20.25v-4.5m0 4.5h4.5m-4.5 0L9 15M20.25 3.75h-4.5m4.5 0v4.5m0-4.5L15 9m5.25 11.25h-4.5m4.5 0v-4.5m0 4.5L15 15",
    MoveCloser => "M9 9V4.5M9 9H4.5M9 9L3.75 3.75M9 15v4.5M9 15H4.5M9 15l-5.25 5.25M15 9h4.5M15 9V4.5M15 9l5.25-5.25M15 15h4.5M15 15v4.5m0-4.5l5.25 5.25",
    Cube => "M21 7.5l-9-5.25L3 7.5m18 0l-9 5.25m9-5.25v9l-9 5.25M3 7.5l9 5.25M3 7.5v9l9 5.25m0-9v9",
    Rotate => "M16.023 9.348h4.992v-.001M2.985 19.644v-4.992m0 0h4.992m-4.993 0l3.181 3.183a8.25 8.25 0 0013.803-3.7M4.031 9.865a8.25 8.25 0 0113.803-3.7l3.181 3.182m0-4.991v4.99",
    FaceOn => "M7.5 21L3 16.5m0 0L7.5 12M3 16.5h13.5m0-13.5L21 7.5m0 0L16.5 12M21 7.5H7.5",
    Bulb => "M12 18v-5.25m0 0a6.01 6.01 0 001.5-.189m-1.5.189a6.01 6.01 0 01-1.5-.189m3.75 7.478a12.06 12.06 0 01-4.5 0m3.75 2.383a14.406 14.406 0 01-3 0M14.25 18v-.192c0-.983.658-1.823 1.508-2.316a7.5 7.5 0 10-7.517 0c.85.493 1.509 1.333 1.509 2.316V18",
    Warning => "M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126zM12 15.75h.007v.008H12v-.008z",
    Crop => "M6 3v13.5A1.5 1.5 0 007.5 18H21M3 6h13.5A1.5 1.5 0 0118 7.5V21",
    Cross => "M9.75 9.75l4.5 4.5m0-4.5l-4.5 4.5M21 12a9 9 0 11-18 0 9 9 0 0118 0z",
}

/// An icon of `size` pixels in `color`.
pub(crate) fn icon(icon: Icon, size: f32, color: Rgba) -> Svg {
    svg()
        .data(icon.svg_data())
        .size(px(size))
        .flex_shrink_0()
        .text_color(color)
}

/// How a banner is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Good,
    Attention,
    Bad,
}

impl Tone {
    /// Background, border and ink.
    fn colors(self, p: Palette) -> (Rgba, Rgba, Rgba) {
        match self {
            Self::Good => (p.success_soft, with_alpha(p.success, 0.4), p.success_ink),
            Self::Attention => (p.warning_soft, with_alpha(p.warning, 0.5), p.warning_ink),
            Self::Bad => (p.danger_soft, with_alpha(p.danger, 0.5), p.danger_ink),
        }
    }
}

pub(crate) fn with_alpha(mut color: Rgba, alpha: f32) -> Rgba {
    color.a = alpha;
    color
}

/// A large status message: an icon, a headline and one sentence of advice.
pub(crate) fn status_banner(
    p: Palette,
    tone: Tone,
    glyph: Icon,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Div {
    let (bg, border, ink) = tone.colors(p);
    div()
        .flex()
        .items_center()
        .gap_4()
        .rounded(px(RADIUS_LG))
        .border_2()
        .border_color(border)
        .bg(bg)
        .px_5()
        .py_4()
        .text_color(ink)
        .child(icon(glyph, 40.0, ink))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.into()),
                )
                .child(div().mt_0p5().text_base().opacity(0.9).child(detail.into())),
        )
}

/// The steps of a flow with the current one highlighted and finished ones
/// ticked. `current` may equal `steps.len()` once everything is done.
pub(crate) fn stepper(p: Palette, steps: &[&'static str], current: usize) -> Div {
    let last = steps.len().saturating_sub(1);
    let mut row = div().flex().items_center().w_full();
    for (i, label) in steps.iter().copied().enumerate() {
        let done = i < current;
        let active = i == current;
        let (dot_bg, dot_fg, dot_border) = if done {
            (p.success_soft, p.success_ink, p.success)
        } else if active {
            (p.accent, p.on_accent, p.accent)
        } else {
            (p.surface, p.fg_muted, p.line_strong)
        };
        let text = if active {
            p.fg_strong
        } else if done {
            p.fg
        } else {
            p.fg_muted
        };
        let dot = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size_8()
            .rounded_full()
            .border_2()
            .border_color(dot_border)
            .bg(dot_bg)
            .text_color(dot_fg)
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD);
        let dot = if done {
            dot.child(icon(Icon::Check, 20.0, dot_fg))
        } else {
            dot.child((i + 1).to_string())
        };
        let mut item = div().flex().items_center().child(
            div().flex().items_center().gap_2().child(dot).child(
                div()
                    .text_sm()
                    .whitespace_nowrap()
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(text)
                    .child(label),
            ),
        );
        if i == last {
            row = row.child(item);
        } else {
            item =
                item.flex_1()
                    .child(div().mx_3().h(px(2.0)).flex_1().rounded_full().bg(if done {
                        p.success
                    } else {
                        p.line_strong
                    }));
            row = row.child(item);
        }
    }
    row
}

/// A labelled slider. The caller owns the [`SliderState`] and listens for its
/// `SliderEvent::Change`; debouncing anything expensive is the caller's job.
pub(crate) fn slider(
    p: Palette,
    state: &Entity<SliderState>,
    label: &'static str,
    value_text: impl Into<SharedString>,
    help: Option<&'static str>,
) -> Div {
    let mut col = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.fg_strong)
                        .child(label),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(p.fg_muted)
                        .child(value_text.into()),
                ),
        )
        .child(SliderWidget::new(state));
    if let Some(help) = help {
        col = col.child(div().text_sm().text_color(p.fg_muted).child(help));
    }
    col
}

const SPINNER: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="3.5" stroke-linecap="round"><path d="M12 3a9 9 0 0 1 9 9"/></svg>"#;

/// A turning arc beside what is being waited for.
pub(crate) fn spinner(size: f32, color: Rgba) -> impl IntoElement {
    svg()
        .data(SPINNER)
        .size(px(size))
        .flex_shrink_0()
        .text_color(color)
        .with_animation(
            "spinner",
            Animation::new(Duration::from_millis(900)).repeat(),
            |s, delta| s.with_transformation(Transformation::rotate(percentage(delta))),
        )
}

/// What the overlay layer draws, in the picture's own pixels: `scale` converts
/// those to screen pixels and `origin` is the layer's top-left.
pub(crate) struct OverlayCanvas {
    pub origin: gpui::Point<Pixels>,
    pub scale: f32,
}

impl OverlayCanvas {
    pub(crate) fn point(&self, x: f32, y: f32) -> gpui::Point<Pixels> {
        gpui::point(
            self.origin.x + px(x * self.scale),
            self.origin.y + px(y * self.scale),
        )
    }

    /// A polyline through `points` (picture pixels), closed when `closed`.
    pub(crate) fn stroke_path(
        &self,
        window: &mut Window,
        points: &[[f32; 2]],
        closed: bool,
        width: f32,
        color: Rgba,
    ) {
        let points: Vec<_> = points.iter().map(|p| self.point(p[0], p[1])).collect();
        let mut builder = PathBuilder::stroke(px(width * self.scale));
        builder.add_polygon(&points, closed);
        // A path that fails to tessellate is a cosmetic loss, not an error.
        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    }

    /// A circle outline around `center` (picture pixels).
    pub(crate) fn stroke_circle(
        &self,
        window: &mut Window,
        center: [f32; 2],
        radius: f32,
        width: f32,
        color: Rgba,
    ) {
        let r = px(radius * self.scale);
        let c = self.point(center[0], center[1]);
        let bounds = Bounds::new(gpui::point(c.x - r, c.y - r), gpui::size(r * 2.0, r * 2.0));
        window.paint_quad(gpui::quad(
            bounds,
            r,
            gpui::transparent_black(),
            px(width * self.scale),
            color,
            BorderStyle::Solid,
        ));
    }
}

/// A picture with a drawing layer on top. `draw` paints in the picture's own
/// pixels (`width` by `height`), so callers place detections without scaling;
/// `labels` are text pieces placed with [`overlay_label`].
pub(crate) fn overlay_image(
    image: Option<Arc<RenderImage>>,
    width: u32,
    height: u32,
    draw: impl FnOnce(&OverlayCanvas, &mut Window) + 'static,
    labels: Vec<AnyElement>,
) -> Div {
    let mut stage = div()
        .relative()
        .w_full()
        .aspect_ratio(width as f32 / height.max(1) as f32)
        .overflow_hidden()
        .rounded(px(RADIUS_LG))
        .bg(gpui::black());
    if let Some(image) = image {
        stage = stage.child(
            img(image)
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Fill),
        );
    }
    stage
        .child(
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, (), window: &mut Window, _: &mut App| {
                    let scale = f32::from(bounds.size.width) / width.max(1) as f32;
                    draw(
                        &OverlayCanvas {
                            origin: bounds.origin,
                            scale,
                        },
                        window,
                    );
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        )
        .children(labels)
}

/// Text at a picture position, sized as a fraction of the picture width.
pub(crate) fn overlay_label(
    x: f32,
    y: f32,
    width: u32,
    height: u32,
    color: Rgba,
    text: String,
) -> AnyElement {
    div()
        .absolute()
        .left(relative(x / width.max(1) as f32))
        .top(relative(y / height.max(1) as f32))
        .text_color(color)
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .child(text)
        .into_any_element()
}

/// Remembers the picture a view last drew so a replaced frame leaves the GPU
/// atlas; a live stream would otherwise keep every frame's texture.
#[derive(Default)]
pub(crate) struct FrameSlot(Option<Arc<RenderImage>>);

impl FrameSlot {
    pub(crate) fn show(&mut self, image: Option<&Arc<RenderImage>>, window: &mut Window) {
        let same = match (&self.0, image) {
            (Some(shown), Some(next)) => Arc::ptr_eq(shown, next),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        if let Some(old) = self.0.take() {
            // Only fails when the image was never uploaded.
            let _ = window.drop_image(old);
        }
        self.0 = image.cloned();
    }
}
