//! Calibrate page: the ways to calibrate a screen. Each method is a wizard in
//! its own window; this page lists them. A new method adds a module beside
//! [`automatic`] and one entry in [`METHODS`].

pub(crate) mod automatic;

use gpui::{
    App, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::components::page::page;
use crate::components::ui;
use crate::components::wizard::{Icon, icon};
use crate::theme::palette;

/// A way to calibrate a screen.
struct Method {
    title: &'static str,
    description: &'static str,
    icon: Icon,
    open: fn(&mut App),
}

const METHODS: [Method; 1] = [Method {
    title: "Automatic (camera)",
    description: "A depth camera looks at the screen and measures its size, position and the tracker markers. Takes a couple of minutes.",
    icon: Icon::Camera,
    open: |cx| automatic::open(None, cx),
}];

pub(crate) struct CalibrateView;

impl CalibrateView {
    pub(crate) fn new(_: &mut Window, _: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for CalibrateView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let methods = METHODS.iter().enumerate().map(move |(i, method)| {
            let open = method.open;
            ui::card(p)
                .id(("calibrate-method", i))
                .flex_row()
                .items_center()
                .gap_4()
                .cursor_pointer()
                .hover(move |style| style.border_color(p.accent_ink))
                .on_click(move |_, _, cx| open(cx))
                .child(icon(method.icon, 40.0, p.accent_ink))
                .child(
                    div()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.fg_strong)
                                .child(method.title),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(p.fg_muted)
                                .child(method.description),
                        ),
                )
                .into_any_element()
        });
        page(
            "calibrate-page",
            p,
            "Calibrate a screen",
            Some(
                "Calibrating teaches the system where your screen is, so shots land where you aim. Choose how to do it."
                    .into(),
            ),
            methods,
        )
    }
}
