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
/// The hub reports an unusable events transport as a plain `Internal` error;
/// surfaced verbatim it reads like a crash, when in fact the device is simply
/// in BLE mode and the register plane lives on the USB bulk endpoint.
pub(super) fn rpc_failure(context: &str, err: &dyn std::fmt::Display) -> OpStatus {
    let raw = format!("{err:#}");
    let summary = if raw.contains("events transport not connected") {
        "Sensor tuning needs the USB data connection. Switch Transport mode to \
         USB in the Connection section, then try again."
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
            span { class: "text-xs text-gray-500 dark:text-gray-400 italic", "{msg}" }
        },
        OpStatus::Done(msg) => rsx! {
            span { class: "text-xs text-green-600 dark:text-green-400", "{msg}" }
        },
        OpStatus::Failed { summary, detail } => rsx! {
            div {
                class: "text-xs text-red-600 dark:text-red-400 space-y-1 basis-full",
                p { "{summary}" }
                if let Some(detail) = detail {
                    details {
                        summary {
                            class: "cursor-pointer text-gray-500 dark:text-gray-400 select-none",
                            "Technical details"
                        }
                        p {
                            class: "mt-1 font-mono text-[11px] text-gray-500 dark:text-gray-400 break-all",
                            "{detail}"
                        }
                    }
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_error_becomes_actionable() {
        let err = anyhow::anyhow!("events transport not connected");
        match rpc_failure("read the sensor settings", &err) {
            OpStatus::Failed { summary, detail } => {
                assert!(summary.contains("USB"), "{summary}");
                assert!(!summary.contains("status:"), "{summary}");
                assert_eq!(detail.unwrap_or_default(), "events transport not connected");
            }
            _ => panic!("expected failure"),
        }
    }
}
