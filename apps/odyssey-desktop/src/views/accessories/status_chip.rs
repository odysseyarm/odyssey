//! The connected/disconnected pill beside an accessory.

use dioxus::prelude::*;

#[component]
pub(super) fn StatusChip(connected: bool) -> Element {
    let (label, dot_cls, pill_cls) = if connected {
        (
            "Connected",
            "h-2.5 w-2.5 rounded-full bg-emerald-500 animate-pulse",
            "bg-emerald-50 text-emerald-700 border-emerald-200",
        )
    } else {
        (
            "Disconnected",
            "h-2.5 w-2.5 rounded-full bg-gray-400",
            "bg-gray-50 text-gray-700 border-gray-200",
        )
    };

    rsx! {
        span { class: "inline-flex items-center gap-2 px-2.5 py-1 rounded-full text-xs border {pill_cls}",
            span { class: dot_cls }
            {label}
        }
    }
}
