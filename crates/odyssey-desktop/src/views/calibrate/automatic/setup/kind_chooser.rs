//! "What are you calibrating?": a flat screen or a projector.

use dioxus::prelude::*;

use crate::components::wizard::{Icon, IconView};
use crate::views::calibrate::automatic::illustrations::{ProjectorIllustration, TvIllustration};
use crate::views::calibrate::automatic::kind::DisplayKind;
use crate::views::calibrate::automatic::wizard::Wizard;

const KINDS: [DisplayKind; 2] = [DisplayKind::Flat, DisplayKind::Projector];

fn kind_title(kind: DisplayKind) -> &'static str {
    match kind {
        DisplayKind::Flat => "TV or monitor",
        DisplayKind::Projector => "Projector",
    }
}

fn kind_blurb(kind: DisplayKind) -> &'static str {
    match kind {
        DisplayKind::Flat => "A flat screen with a fixed size",
        DisplayKind::Projector => "A picture thrown onto a wall or screen",
    }
}

fn kind_id(kind: DisplayKind) -> &'static str {
    match kind {
        DisplayKind::Flat => "kind-flat",
        DisplayKind::Projector => "kind-projector",
    }
}

/// Two big illustrated cards behind one radio group. Arrow keys move the
/// choice, and only the chosen card is a tab stop, as for any radio group.
#[component]
pub(super) fn KindChooser(wizard: Wizard) -> Element {
    let chosen = wizard.kind();
    rsx! {
        div {
            role: "radiogroup",
            aria_label: "What are you calibrating?",
            class: "grid grid-cols-1 gap-3 sm:grid-cols-2",
            onkeydown: move |e| {
                let step = match e.key() {
                    Key::ArrowRight | Key::ArrowDown => 1,
                    Key::ArrowLeft | Key::ArrowUp => KINDS.len() - 1,
                    _ => return,
                };
                e.prevent_default();
                let at = KINDS.iter().position(|&k| k == wizard.kind()).unwrap_or(0);
                let next = KINDS[(at + step) % KINDS.len()];
                wizard.set_kind(next);
                let _ = document::eval(&format!("document.getElementById('{}')?.focus();", kind_id(next)));
            },
            for kind in KINDS {
                {
                    let selected = kind == chosen;
                    let class = if selected {
                        "border-blue-600 bg-blue-50 ring-4 ring-blue-200 dark:border-blue-400 dark:bg-blue-950/50 dark:ring-blue-900"
                    } else {
                        "border-gray-300 bg-white hover:border-blue-400 dark:border-gray-500 dark:bg-gray-800"
                    };
                    rsx! {
                        button {
                            key: "{kind_id(kind)}",
                            id: kind_id(kind),
                            r#type: "button",
                            role: "radio",
                            aria_checked: selected,
                            tabindex: if selected { "0" } else { "-1" },
                            class: "relative flex flex-col items-center gap-2 rounded-2xl border-2 px-4 pb-4 pt-5 text-center transition-colors focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:focus-visible:ring-blue-800 {class}",
                            onclick: move |_| wizard.set_kind(kind),
                            if selected {
                                span {
                                    class: "absolute right-3 top-3 text-blue-600 dark:text-blue-300",
                                    IconView { icon: Icon::Check, class: "size-7".to_string() }
                                }
                            }
                            match kind {
                                DisplayKind::Flat => rsx! { TvIllustration { class: "h-28".to_string() } },
                                DisplayKind::Projector => rsx! { ProjectorIllustration { class: "h-28".to_string() } },
                            }
                            span { class: "text-xl font-semibold text-gray-900 dark:text-white", "{kind_title(kind)}" }
                            span { class: "text-base text-gray-600 dark:text-gray-300", "{kind_blurb(kind)}" }
                        }
                    }
                }
            }
        }
    }
}
