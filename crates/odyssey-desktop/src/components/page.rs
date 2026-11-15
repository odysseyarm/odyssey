//! The frame every sidebar page sits in, so they share one width, one heading
//! style and one background. Scrolls when the content is taller than the window.

use gpui::{
    AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div,
};

use crate::components::ui;
use crate::theme::Palette;

/// `description` is one or two sentences under the title saying what the page
/// is for.
pub(crate) fn page(
    id: &'static str,
    p: &Palette,
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    children: impl IntoIterator<Item = AnyElement>,
) -> impl IntoElement {
    div()
        .id(id)
        .size_full()
        .overflow_y_scroll()
        .child(ui::page(p, title, description, children))
}
