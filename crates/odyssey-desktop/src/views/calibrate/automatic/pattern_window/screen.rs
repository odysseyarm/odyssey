//! The page inside the pattern window.

use std::cell::Cell;
use std::rc::Rc;

use dioxus::prelude::*;
use tokio::sync::mpsc::UnboundedSender;

use super::display::Link;
use super::mode::{CARD_BIG_STEPS, PatternEvent, PatternLoss, PatternMode};
use super::pictures::{CardPicture, Countdown, PatternPicture, RulerPicture};
use crate::components::Theme;

#[component]
pub(super) fn PatternScreen(link: Link) -> Element {
    let mut mode = use_signal(|| *link.0.mode.borrow());
    // A plain cell rather than a signal: cleanup may run after the scope's
    // signals are gone.
    let wizard_closed_it = use_hook(|| Rc::new(Cell::new(false)));

    use_future({
        let link = link.clone();
        let wizard_closed_it = Rc::clone(&wizard_closed_it);
        move || {
            let mut receiver = link.0.mode.clone();
            let wizard_closed_it = Rc::clone(&wizard_closed_it);
            async move {
                // `changed` fails once the wizard drops its display, which is
                // the wizard's way of closing this window.
                while receiver.changed().await.is_ok() {
                    mode.set(*receiver.borrow_and_update());
                }
                wizard_closed_it.set(true);
                dioxus::desktop::window().close();
            }
        }
    });

    use_drop({
        let link = link.clone();
        move || {
            if !wizard_closed_it.get() {
                let _ = link.0.events.send(PatternEvent::Lost(PatternLoss::Closed));
            }
        }
    });

    let (screen_w, screen_h) = (f64::from(link.0.pixels.0), f64::from(link.0.pixels.1));
    let events = link.0.events.clone();

    rsx! {
        Theme {}

        div {
            class: "fixed inset-0 bg-black select-none overflow-hidden outline-none",
            style: "cursor: none;",
            tabindex: 0,
            onmounted: move |e| async move {
                let _ = e.set_focus(true).await;
            },
            onkeydown: move |e| handle_key(&e.key(), mode(), &events),
            match mode() {
                PatternMode::Pattern => rsx! { PatternPicture { link: link.clone(), screen_w, screen_h } },
                PatternMode::Black => rsx! {},
                PatternMode::Ruler { line_px, covers_wizard } => rsx! {
                    RulerPicture { line_px, covers_wizard, screen_w, screen_h }
                },
                PatternMode::Card { width_px, covers_wizard } => rsx! {
                    CardPicture { width_px, covers_wizard, screen_w, screen_h }
                },
                PatternMode::Countdown(seconds) => rsx! { Countdown { seconds } },
            }
        }
    }
}

/// Escape closes the window; Enter does too, but only on a helper picture,
/// where it is the way back to the wizard. The arrow and page keys resize the card.
fn handle_key(key: &Key, mode: PatternMode, events: &UnboundedSender<PatternEvent>) {
    let helper_picture = matches!(mode, PatternMode::Ruler { .. } | PatternMode::Card { .. });
    let card = matches!(mode, PatternMode::Card { .. });
    let nudge = |steps: i32| {
        let _ = events.send(PatternEvent::Nudge(steps));
    };
    match key {
        Key::Escape => dioxus::desktop::window().close(),
        // Coming back from a helper picture; on the calibration pattern
        // Enter must not end it.
        Key::Enter if helper_picture => dioxus::desktop::window().close(),
        Key::ArrowUp | Key::ArrowRight if card => nudge(1),
        Key::ArrowDown | Key::ArrowLeft if card => nudge(-1),
        Key::PageUp if card => nudge(CARD_BIG_STEPS),
        Key::PageDown if card => nudge(-CARD_BIG_STEPS),
        _ => {}
    }
}
