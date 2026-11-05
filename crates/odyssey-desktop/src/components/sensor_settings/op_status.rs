use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement as _, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};

use crate::theme::Palette;

/// Outcome of the most recent user-triggered operation in a panel.
///
/// `Failed` keeps the customer-facing sentence separate from the raw error so
/// the latter can stay collapsed; support still gets the exact text.
#[derive(Clone, PartialEq, Default)]
pub(super) enum OpStatus {
    #[default]
    Idle,
    Busy(String),
    Done(String),
    Failed {
        summary: String,
        detail: Option<String>,
    },
}

/// Turn a transport/RPC failure into something a customer can act on.
///
/// Register access needs an active events connection; a USB control interface
/// alone does not imply that sensor data is connected.
pub(super) fn rpc_failure(context: &str, err: &dyn std::fmt::Display) -> OpStatus {
    let raw = format!("{err:#}");
    let summary = if raw.contains("events transport not connected") {
        "Sensor tuning needs an active sensor data connection. Connect over USB \
         or Bluetooth, then try again."
            .to_string()
    } else if raw.contains("not connected to the hub") || raw.contains("transport error") {
        "Lost the connection to the Odyssey service.".to_string()
    } else if raw.contains("device not found") {
        "The device disconnected before the request finished.".to_string()
    } else {
        format!("Could not {context}.")
    };
    OpStatus::Failed {
        summary,
        detail: Some(raw),
    }
}

/// The status of the last operation. `id` must be unique among the status
/// lines in the window; `details_open` and `on_toggle` drive the collapsed
/// technical details of a failure.
pub(super) fn status_line(
    status: &OpStatus,
    id: String,
    details_open: bool,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    palette: Palette,
) -> AnyElement {
    match status {
        OpStatus::Idle => div().into_any_element(),
        OpStatus::Busy(message) => div()
            .text_xs()
            .italic()
            .text_color(palette.fg_muted)
            .child(message.clone())
            .into_any_element(),
        OpStatus::Done(message) => div()
            .text_xs()
            .text_color(palette.success_ink)
            .child(message.clone())
            .into_any_element(),
        OpStatus::Failed { summary, detail } => {
            let mut failure = div()
                .w_full()
                .flex()
                .flex_col()
                .gap_1()
                .text_xs()
                .text_color(palette.danger_ink)
                .child(summary.clone());
            if let Some(detail) = detail {
                failure = failure
                    .child(
                        div()
                            .id(id)
                            .cursor_pointer()
                            .text_color(palette.fg_muted)
                            .on_click(on_toggle)
                            .child(if details_open {
                                "▾ Technical details"
                            } else {
                                "▸ Technical details"
                            }),
                    )
                    .when(details_open, |this| {
                        this.child(
                            div()
                                .mt_1()
                                .font_family("monospace")
                                .text_size(gpui::px(11.0))
                                .text_color(palette.fg_muted)
                                .child(detail.clone()),
                        )
                    });
            }
            failure.into_any_element()
        }
    }
}
