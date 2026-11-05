//! One device's row in the sidebar: what to zero on, and the zero and shot
//! delay controls.

use gpui::component::input::{InputEvent, InputState};
use gpui::{
    AppContext as _, Context, Div, ElementId, Entity, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
};
use odyssey_hub_client::client::Client;
use odyssey_hub_common::device::Device;

use super::device_state::DeviceState;
use crate::hub::Hub;
use crate::runtime;
use crate::theme::{Palette, palette};

/// How a [`pill`] button is tinted.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Plain,
    Success,
    Danger,
}

/// The sidebar's small secondary button (`btn btn-secondary btn-sm`). The
/// caller attaches the click handler and ignores clicks while `disabled`.
pub(super) fn pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    tone: Tone,
    disabled: bool,
    p: Palette,
) -> Stateful<Div> {
    let (ink, border): (_, Hsla) = match tone {
        Tone::Plain => (p.fg, p.line_strong.into()),
        Tone::Success => (p.success_ink, Hsla::from(p.success).opacity(0.4)),
        Tone::Danger => (p.danger_ink, p.line_strong.into()),
    };
    let hover_border: Hsla = if tone == Tone::Danger {
        p.danger_ink.into()
    } else {
        border
    };
    let pill = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .rounded_full()
        .border_1()
        .border_color(border)
        .bg(p.surface)
        .text_color(ink)
        .px_3()
        .py_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(label.into());
    if disabled {
        return pill.opacity(0.5);
    }
    pill.cursor_pointer()
        .hover(move |style| style.bg(p.surface_hover).border_color(hover_border))
}

pub(super) struct DeviceControls {
    pub(super) device: Device,
    pub(super) hub: Entity<Hub>,
    pub(super) state: DeviceState,
    pub(super) shot_delay_input: Entity<InputState>,
    /// The `DeviceState` shot-delay revision the input text last showed.
    pub(super) shown_revision: u64,
    /// A shot-delay command is in flight; the controls wait for it.
    pub(super) pending: bool,
    pub(super) saving: bool,
    /// Clears the save status after a pause. A new status replaces the task,
    /// so an earlier timer can never clear a newer result too early.
    pub(super) save_status_timer: Option<Task<()>>,
    _input_events: Subscription,
}

impl DeviceControls {
    pub(super) fn new(
        device: Device,
        hub: Entity<Hub>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = DeviceState::default();
        let shot_delay_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(state.shot_delay_ms().to_string())
                .validate(|typed, _| typed.chars().all(|c| c.is_ascii_digit()))
        });
        let input_events = cx.subscribe_in(&shot_delay_input, window, |controls, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                controls.shot_delay_typed(window, cx);
            }
        });
        Self {
            device,
            hub,
            shown_revision: state.shot_delay_revision(),
            state,
            shot_delay_input,
            pending: false,
            saving: false,
            save_status_timer: None,
            _input_events: input_events,
        }
    }

    pub(super) fn set_device(&mut self, device: Device) {
        self.device = device;
    }

    /// The connected client, or a logged warning that the hub is unreachable.
    pub(super) fn client(&self, action: &str, cx: &gpui::App) -> Option<Client> {
        let client = self.hub.read(cx).client();
        if client.is_none() {
            tracing::warn!(
                "Failed to {action} {:x?}: not connected to the hub",
                self.device.uuid
            );
        }
        client
    }

    fn reset_zero(&self, cx: &gpui::App) {
        let Some(client) = self.client("reset zero", cx) else {
            return;
        };
        let device = self.device.clone();
        runtime::spawn(async move {
            match client.device(device.clone()).reset_zero().await {
                Ok(()) => tracing::info!("Cleared zero for {:x?}", device.uuid),
                Err(e) => tracing::error!("Failed to reset zero {:x?}: {}", device.uuid, e),
            }
        });
    }

    fn clear_zero(&self, cx: &gpui::App) {
        let Some(client) = self.client("clear zero", cx) else {
            return;
        };
        let device = self.device.clone();
        runtime::spawn(async move {
            match client.device(device.clone()).clear_zero().await {
                Ok(()) => tracing::info!("Cleared zero for {:x?}", device.uuid),
                Err(e) => tracing::error!("Failed to clear zero {:x?}: {}", device.uuid, e),
            }
        });
    }
}

impl Render for DeviceControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        self.show_shot_delay(window, cx);
        let id = cx.entity_id();
        let firing = self.state.shooting;
        let zeroing_on_key = self.state.zeroing_on_key;

        let shot_button = if firing {
            pill(("zero-cancel-shot", id), "Cancel", Tone::Danger, false, p)
        } else {
            pill(("zero-on-shot", id), "Zero on shot", Tone::Plain, false, p)
        }
        .on_click(cx.listener(move |controls, _, _, cx| {
            controls.state.shooting = !firing;
            cx.notify();
        }));
        let key_button = pill(
            ("zero-on-key", id),
            if zeroing_on_key { "Cancel [Z]" } else { "Zero on [Z]" },
            if zeroing_on_key { Tone::Danger } else { Tone::Plain },
            false,
            p,
        )
        .on_click(cx.listener(|controls, _, _, cx| {
            controls.state.zeroing_on_key = !controls.state.zeroing_on_key;
            cx.notify();
        }));
        let reset_button = pill(("zero-reset", id), "Reset Zero", Tone::Plain, false, p)
            .on_click(cx.listener(|controls, _, _, cx| controls.reset_zero(cx)));
        let clear_button = pill(("zero-clear", id), "Clear Zero", Tone::Plain, false, p)
            .on_click(cx.listener(|controls, _, _, cx| controls.clear_zero(cx)));

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_color(p.fg_strong)
                    .child(crate::views::device_label(&self.device)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .flex_wrap()
                            .child(shot_button)
                            .child(key_button)
                            .child(reset_button)
                            .child(clear_button)
                            .child(self.save_zero_button(p, cx)),
                    )
                    .child(self.shot_delay_row(p, cx)),
            )
    }
}
