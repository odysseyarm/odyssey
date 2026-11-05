//! The connected/disconnected pill beside an accessory.

use dioxus::prelude::*;

#[component]
pub(super) fn StatusChip(connected: bool) -> Element {
    let (label, dot_cls, badge_cls) = if connected {
        (
            "Connected",
            "size-2 rounded-full bg-success animate-pulse",
            "badge-success",
        )
    } else {
        (
            "Disconnected",
            "size-2 rounded-full bg-fg-faint",
            "badge-neutral",
        )
    };

    rsx! {
        span { class: "badge {badge_cls}",
            span { class: dot_cls }
            {label}
        }
    }
}
