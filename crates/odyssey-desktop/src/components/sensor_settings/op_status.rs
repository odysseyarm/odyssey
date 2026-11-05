use dioxus::prelude::*;

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

#[component]
pub(super) fn StatusLine(status: OpStatus) -> Element {
    match status {
        OpStatus::Idle => rsx! {},
        OpStatus::Busy(msg) => rsx! {
            span { class: "text-xs text-fg-muted italic", {msg} }
        },
        OpStatus::Done(msg) => rsx! {
            span { class: "text-xs text-success-ink", {msg} }
        },
        OpStatus::Failed { summary, detail } => rsx! {
            div {
                class: "text-xs text-danger-ink space-y-1 basis-full",
                p { {summary} }
                if let Some(detail) = detail {
                    details {
                        summary {
                            class: "cursor-pointer text-fg-muted select-none",
                            "Technical details"
                        }
                        p {
                            class: "mt-1 font-mono text-[11px] text-fg-muted break-all",
                            {detail}
                        }
                    }
                }
            }
        },
    }
}
