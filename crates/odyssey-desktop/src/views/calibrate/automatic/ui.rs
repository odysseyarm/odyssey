//! Layout pieces shared by the wizard's step views.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, ElementId, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use super::wizard::Wizard;
use crate::components::ui::{AlertKind, alert};
use crate::components::wizard::{Icon, icon, spinner};
use crate::theme::{Palette, RADIUS, RADIUS_LG};

pub(super) type Handler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A click handler that runs `f` on the wizard.
pub(super) fn act(
    wizard: &Entity<Wizard>,
    f: impl Fn(&mut Wizard, &mut Context<Wizard>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let wizard = wizard.clone();
    move |_, _, cx| wizard.update(cx, |wizard, cx| f(wizard, cx))
}

/// `act`, boxed for lists of choices.
pub(super) fn act_boxed(
    wizard: &Entity<Wizard>,
    f: impl Fn(&mut Wizard, &mut Context<Wizard>) + 'static,
) -> Handler {
    Box::new(act(wizard, f))
}

/// One step's page: a headline, one line of instruction, the content and a
/// footer row of buttons.
pub(super) fn step_page(
    p: Palette,
    title: impl Into<SharedString>,
    subtitle: Option<SharedString>,
    content: Vec<AnyElement>,
    footer: Vec<AnyElement>,
) -> Div {
    let mut head = div().child(
        div()
            .text_3xl()
            .font_weight(FontWeight::BOLD)
            .text_color(p.fg_strong)
            .child(title.into()),
    );
    if let Some(subtitle) = subtitle {
        head = head.child(
            div()
                .mt_1()
                .text_lg()
                .text_color(p.fg_muted)
                .child(subtitle),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap_5()
        .child(head)
        .children(content)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .pt_2()
                .children(footer),
        )
}

/// A section heading inside a card.
pub(super) fn heading(p: Palette, text: impl Into<SharedString>) -> Div {
    div()
        .text_xl()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.fg_strong)
        .child(text.into())
}

/// The open/closed state for one [`details`]; fetch it before borrowing the
/// model from `cx`. `id` must be unique among the details of one view.
pub(super) fn open_state(id: &'static str, window: &mut Window, cx: &mut App) -> Entity<bool> {
    window.use_keyed_state(id, cx, |_, _| false)
}

/// Technical information, collapsed until asked for.
pub(super) fn details(
    id: &'static str,
    p: Palette,
    summary: &'static str,
    body: Vec<AnyElement>,
    open: &Entity<bool>,
    cx: &App,
) -> Div {
    let is_open = *open.read(cx);
    let open = open.clone();
    let mut el = div()
        .rounded(px(RADIUS_LG))
        .border_1()
        .border_color(p.line)
        .bg(p.surface_alt)
        .px_4()
        .py_3()
        .child(
            div()
                .id(ElementId::Name(format!("{id}-summary").into()))
                .cursor_pointer()
                .text_base()
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.fg)
                .child(format!(
                    "{} {summary}",
                    if is_open { "\u{25be}" } else { "\u{25b8}" }
                ))
                .on_click(move |_, _, cx| {
                    open.update(cx, |open, cx| {
                        *open = !*open;
                        cx.notify();
                    });
                }),
        );
    if is_open {
        el = el.child(
            div()
                .mt_3()
                .flex()
                .flex_col()
                .gap_2()
                .text_sm()
                .text_color(p.fg)
                .children(body),
        );
    }
    el
}

/// A lighter collapsible for hints under a card, without the box.
pub(super) fn plain_details(
    id: &'static str,
    p: Palette,
    summary: &'static str,
    body: Vec<AnyElement>,
    open: &Entity<bool>,
    cx: &App,
) -> Div {
    let is_open = *open.read(cx);
    let open = open.clone();
    let mut el = div().text_sm().text_color(p.fg_muted).child(
        div()
            .id(ElementId::Name(format!("{id}-summary").into()))
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM)
            .child(format!(
                "{} {summary}",
                if is_open { "\u{25be}" } else { "\u{25b8}" }
            ))
            .on_click(move |_, _, cx| {
                open.update(cx, |open, cx| {
                    *open = !*open;
                    cx.notify();
                });
            }),
    );
    if is_open {
        el = el.child(div().mt_2().flex().flex_col().gap_2().children(body));
    }
    el
}

/// A label and value on one line inside [`details`].
pub(super) fn fact(
    p: Palette,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> AnyElement {
    div()
        .flex()
        .justify_between()
        .gap_4()
        .child(div().text_color(p.fg_muted).child(label.into()))
        .child(div().child(value.into()))
        .into_any_element()
}

/// A text-only button that reads as a link.
pub(super) fn link_button(
    id: impl Into<ElementId>,
    p: Palette,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .w_auto()
        .cursor_pointer()
        .text_base()
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.accent_ink)
        .hover(|s| s.underline())
        .child(label.into())
        .on_click(on_click)
}

/// A one-line warning box.
pub(super) fn amber_alert(p: Palette, text: impl Into<SharedString>) -> AnyElement {
    alert(AlertKind::Warning, p, div().text_base().child(text.into())).into_any_element()
}

/// A spinner beside what is being waited for.
pub(super) fn working_row(p: Palette, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .text_base()
        .text_color(p.fg_muted)
        .child(spinner(20.0, p.fg_muted))
        .child(text.into())
}

/// A one-line hint box with an icon.
pub(super) fn info_note(p: Palette, body: Div) -> AnyElement {
    alert(
        AlertKind::Info,
        p,
        div()
            .flex()
            .items_start()
            .gap_3()
            .text_base()
            .child(div().mt_0p5().child(icon(Icon::Bulb, 24.0, p.info_ink)))
            .child(body),
    )
    .into_any_element()
}

/// The empty stage where the live camera picture goes.
pub(super) fn camera_placeholder(p: Palette, text: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w_full()
        .aspect_ratio(16.0 / 9.0)
        .rounded(px(RADIUS_LG))
        .bg(gpui::black())
        .text_color(p.fg_faint)
        .child(text)
}

/// A bordered group of mutually exclusive options.
pub(super) fn segmented(
    p: Palette,
    id: &'static str,
    items: Vec<(SharedString, bool, Handler)>,
    pad_x: f32,
) -> Div {
    let mut row = div()
        .flex()
        .w_auto()
        .overflow_hidden()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(p.line_strong);
    for (i, (label, selected, on_click)) in items.into_iter().enumerate() {
        let (bg, fg) = if selected {
            (p.accent, p.on_accent)
        } else {
            (p.surface, p.fg)
        };
        row = row.child(
            div()
                .id(ElementId::from((id, i)))
                .px(px(pad_x))
                .py_2()
                .text_base()
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .bg(bg)
                .text_color(fg)
                .when(!selected, |s| s.hover(move |s| s.bg(p.surface_hover)))
                .child(label)
                .on_click(move |e, w, cx| on_click(e, w, cx)),
        );
    }
    row
}
