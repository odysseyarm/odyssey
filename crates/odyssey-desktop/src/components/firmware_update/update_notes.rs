use dioxus::prelude::*;

/// Where Odyssey Desktop releases are published.
pub(super) const DESKTOP_RELEASES_URL: &str = "https://github.com/odysseyarm/odyssey/releases";

/// A firmware release newer than the one on offer that this desktop is too old
/// to install. The user decides whether to update the desktop.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HeldBack {
    pub version: String,
    pub min_desktop: String,
}

pub(super) fn held_back_note(held_back: &HeldBack) -> Element {
    // Offline mode forbids offering a link to the web.
    let offline = odyssey_hub_server::policy::offline();
    rsx! {
        span {
            class: "text-xs text-fg-muted italic",
            "v{held_back.version} needs Odyssey Desktop {held_back.min_desktop} or newer. "
            if offline {
                "Ask your Odyssey provider for it."
            } else {
                a {
                    class: "link",
                    href: DESKTOP_RELEASES_URL,
                    "Get the latest"
                }
            }
        }
    }
}

/// A newer firmware release this unit cannot take yet because it must first run
/// a newer net core than it does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NetCoreBlocked {
    pub version: String,
}

/// `bridge` is the release on offer that carries the net core, when there is one.
pub(super) fn net_core_blocked_note(blocked: &NetCoreBlocked, bridge: Option<&str>) -> Element {
    rsx! {
        span {
            class: "text-xs text-fg-muted italic",
            if let Some(bridge) = bridge {
                "v{blocked.version} needs a newer net core; update to v{bridge} first."
            } else {
                "No update path from this unit's net core (v{blocked.version} needs a newer one)."
            }
        }
    }
}
