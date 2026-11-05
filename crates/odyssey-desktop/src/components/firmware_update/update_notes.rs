use gpui::{
    AnyElement, InteractiveElement as _, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div,
};

use crate::theme::Palette;

/// Where Odyssey Desktop releases are published.
pub(super) const DESKTOP_RELEASES_URL: &str = "https://github.com/odysseyarm/odyssey/releases";

/// A firmware release newer than the one on offer that this desktop is too old
/// to install. The user decides whether to update the desktop.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HeldBack {
    pub version: String,
    pub min_desktop: String,
}

pub(super) fn held_back_note(held_back: &HeldBack, p: Palette) -> AnyElement {
    // Offline mode forbids offering a link to the web.
    let offline = odyssey_hub_server::policy::offline();
    let mut note = div()
        .flex()
        .gap_1()
        .text_xs()
        .italic()
        .text_color(p.fg_muted)
        .child(format!(
            "v{} needs Odyssey Desktop {} or newer.",
            held_back.version, held_back.min_desktop
        ));
    note = if offline {
        note.child("Ask your Odyssey provider for it.")
    } else {
        note.child(
            div()
                .id("desktop-releases-link")
                .cursor_pointer()
                .not_italic()
                .underline()
                .text_color(p.accent_ink)
                .on_click(|_, _, cx| cx.open_url(DESKTOP_RELEASES_URL))
                .child("Get the latest"),
        )
    };
    note.into_any_element()
}

/// A newer firmware release this unit cannot take yet because it must first run
/// a newer net core than it does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NetCoreBlocked {
    pub version: String,
}

/// `bridge` is the release on offer that carries the net core, when there is one.
pub(super) fn net_core_blocked_note(
    blocked: &NetCoreBlocked,
    bridge: Option<&str>,
    p: Palette,
) -> AnyElement {
    let text = match bridge {
        Some(bridge) => format!(
            "v{} needs a newer net core; update to v{bridge} first.",
            blocked.version
        ),
        None => format!(
            "No update path from this unit's net core (v{} needs a newer one).",
            blocked.version
        ),
    };
    div()
        .text_xs()
        .italic()
        .text_color(p.fg_muted)
        .child(text)
        .into_any_element()
}
