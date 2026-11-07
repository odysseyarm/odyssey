//! Choosing the screen, then calibrating it or zeroing on it.

use gpui::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _, div};

use super::{HomeEvent, HomeView};
use crate::components::ui::{self, ButtonKind, button};
use crate::theme::Palette;

impl HomeView {
    pub(super) fn screen_actions(&self, p: Palette, cx: &mut Context<'_, Self>) -> AnyElement {
        let no_devices = self.hub.read(cx).devices.is_empty();
        let selected = self.selection.read(cx).selected_key();
        let selection = self.selection.clone();
        ui::card(p)
            .gap_3()
            .child(self.picker.clone())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_3()
                    .child(
                        button(
                            "calibrate-screen",
                            "Calibrate screen",
                            ButtonKind::Secondary,
                            p,
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(HomeEvent::OpenCalibrate))),
                    )
                    .child(
                        button("zero", "Zero", ButtonKind::Primary, p)
                            .disabled(no_devices || selected.is_none())
                            .on_click(move |_, _, cx| {
                                if let Some(monitor) = selection.read(cx).selected_key() {
                                    crate::views::zero::open(monitor, cx);
                                }
                            }),
                    ),
            )
            .into_any_element()
    }
}
