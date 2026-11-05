//! The Pair or Cancel button and the clear-bonds button of a pairing row.

use dioxus::prelude::*;

const BUTTON: &str = "btn btn-secondary btn-sm whitespace-nowrap";

#[component]
pub(super) fn PairButtons(
    pairing: bool,
    command_pending: bool,
    clear_label: &'static str,
    /// The question the clear button asks before it acts.
    clear_prompt: &'static str,
    on_pair: EventHandler<()>,
    on_cancel: EventHandler<()>,
    on_clear: EventHandler<()>,
) -> Element {
    // Clearing forgets every bond, and re-pairing each device takes a trip to
    // it, so the first click only asks.
    let mut confirming_clear = use_signal(|| false);

    if confirming_clear() {
        return rsx! {
            span { class: "text-xs text-fg", {clear_prompt} }
            button {
                class: "btn btn-danger btn-sm whitespace-nowrap",
                disabled: command_pending,
                onclick: move |_| {
                    confirming_clear.set(false);
                    on_clear.call(());
                },
                {clear_label}
            }
            button {
                class: BUTTON,
                onclick: move |_| confirming_clear.set(false),
                "Keep"
            }
        };
    }

    rsx! {
        if pairing {
            button {
                class: BUTTON,
                disabled: command_pending,
                onclick: move |_| on_cancel.call(()),
                "Cancel"
            }
        } else {
            button {
                class: BUTTON,
                disabled: command_pending,
                onclick: move |_| on_pair.call(()),
                "Pair"
            }
        }
        button {
            class: BUTTON,
            disabled: command_pending,
            onclick: move |_| confirming_clear.set(true),
            {clear_label}
        }
    }
}
