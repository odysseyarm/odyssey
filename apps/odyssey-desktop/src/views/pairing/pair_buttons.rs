//! The Pair or Cancel button and the clear-bonds button of a pairing row.

use dioxus::prelude::*;

const BUTTON: &str = "btn-secondary text-xs";

#[component]
pub(super) fn PairButtons(
    pairing: bool,
    command_pending: bool,
    clear_label: &'static str,
    on_pair: EventHandler<()>,
    on_cancel: EventHandler<()>,
    on_clear: EventHandler<()>,
) -> Element {
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
            onclick: move |_| on_clear.call(()),
            {clear_label}
        }
    }
}
