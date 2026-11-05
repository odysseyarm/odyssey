//! Small shared GPUI building blocks: `card`, `button`, `badge`, `alert`, `progress`.

use gpui::{
    AnyElement, App, ClickEvent, CursorStyle, Div, ElementId, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px, relative,
};

use crate::theme::{Palette, RADIUS_LG, RADIUS_XL};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonKind {
    Primary,
    Secondary,
    Danger,
    Ghost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlertKind {
    Info,
    Success,
    Warning,
    Danger,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A pill button (`btn`). Build it with [`button`].
#[derive(IntoElement)]
pub(crate) struct Button {
    id: ElementId,
    label: SharedString,
    kind: ButtonKind,
    palette: Palette,
    disabled: bool,
    small: bool,
    on_click: Option<ClickHandler>,
}

pub(crate) fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    palette: Palette,
) -> Button {
    Button {
        id: id.into(),
        label: label.into(),
        kind,
        palette,
        disabled: false,
        small: false,
        on_click: None,
    }
}

impl Button {
    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The compact `btn-sm` size.
    pub(crate) fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub(crate) fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let p = self.palette;
        let (bg, fg, border, hover) = match self.kind {
            ButtonKind::Primary => (p.accent, p.on_accent, p.accent, p.accent_strong),
            ButtonKind::Secondary => (p.surface, p.fg, p.line_strong, p.surface_hover),
            ButtonKind::Danger => (p.danger, p.on_danger, p.danger, p.danger),
            ButtonKind::Ghost => (p.surface, p.fg_muted, p.surface, p.surface_hover),
        };
        let mut el = div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .rounded_full()
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .font_weight(FontWeight::SEMIBOLD)
            .child(self.label);
        el = if self.small {
            el.px_3().py_1().text_xs()
        } else {
            el.px_5().py_2p5().text_sm()
        };
        if self.kind == ButtonKind::Ghost {
            el = el.border_0();
        }
        if self.disabled {
            return el.opacity(0.5).cursor(CursorStyle::OperationNotAllowed);
        }
        el = el.cursor_pointer().hover(move |s| s.bg(hover));
        match self.on_click {
            Some(handler) => el.on_click(move |event, window, cx| handler(event, window, cx)),
            None => el,
        }
    }
}

/// `card`: rounded-xl border bg-surface p-5.
pub(crate) fn card(p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(RADIUS_LG))
        .border_1()
        .border_color(p.line)
        .bg(p.surface)
        .p_5()
}

/// `card-title`.
pub(crate) fn card_title(p: Palette, text: impl Into<SharedString>) -> Div {
    div()
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.fg_strong)
        .child(text.into())
}

/// A tinted, bordered message box.
pub(crate) fn alert(kind: AlertKind, p: Palette, content: impl IntoElement) -> impl IntoElement {
    let (bg, ink) = tint(kind, p);
    div()
        .rounded(px(RADIUS_XL))
        .border_1()
        .border_color(ink)
        .bg(bg)
        .text_color(ink)
        .px_4()
        .py_3()
        .text_sm()
        .child(content)
}

fn tint(kind: AlertKind, p: Palette) -> (gpui::Rgba, gpui::Rgba) {
    match kind {
        AlertKind::Info => (p.info_soft, p.info_ink),
        AlertKind::Success => (p.success_soft, p.success_ink),
        AlertKind::Warning => (p.warning_soft, p.warning_ink),
        AlertKind::Danger => (p.danger_soft, p.danger_ink),
    }
}

/// A horizontal progress bar filled to `percent` (clamped to 100).
pub(crate) fn progress_bar(p: Palette, percent: usize) -> impl IntoElement {
    div()
        .h(px(8.0))
        .w_full()
        .overflow_hidden()
        .rounded_full()
        .bg(p.line)
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(p.accent)
                .w(relative(percent.min(100) as f32 / 100.0)),
        )
}

/// The frame every sidebar page sits in: one width, one heading style, one
/// background.
pub(crate) fn page(
    p: Palette,
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    children: impl IntoIterator<Item = AnyElement>,
) -> impl IntoElement {
    let mut header = div().flex().flex_col().gap_1().child(
        div()
            .text_2xl()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(p.fg_strong)
            .child(title.into()),
    );
    if let Some(description) = description {
        header = header.child(div().text_sm().text_color(p.fg_muted).child(description));
    }
    div().w_full().min_h_full().bg(p.canvas).px_6().py_8().child(
        div()
            .mx_auto()
            .w_full()
            .max_w(px(672.0))
            .flex()
            .flex_col()
            .gap_4()
            .child(header)
            .children(children),
    )
}
