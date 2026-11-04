//! The Pair or Cancel button and the clear-bonds button of a pairing row.

use dioxus::prelude::*;

const BUTTON: &str = "btn-secondary text-xs whitespace-nowrap";
const DANGER_BUTTON: &str = "py-2.5 px-4 text-xs whitespace-nowrap rounded-lg font-medium text-white bg-red-600 hover:bg-red-700 focus:outline-none focus:ring-4 focus:ring-red-300 dark:focus:ring-red-900 disabled:opacity-50";

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
            span { class: "text-xs text-gray-700 dark:text-gray-200", {clear_prompt} }
            button {
                class: DANGER_BUTTON,
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
