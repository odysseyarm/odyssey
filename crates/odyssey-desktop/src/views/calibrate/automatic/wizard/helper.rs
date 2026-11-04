//! The tape and card helper: a line or card the tech measures by hand.

use dioxus::prelude::*;
use odyssey_autocalib::session::KnownSize;
use odyssey_hub_common::config::MonitorKey;

use super::Wizard;
use crate::components::screen_picker::monitor_key;
use crate::views::calibrate::automatic::kind::DisplayKind;
use crate::views::calibrate::automatic::pattern_window::{self, PatternMode};
use crate::views::calibrate::automatic::reference::{
    self, Method, Reference, ReferenceError, Unit,
};

/// The tape and card helper while it is open. The monitor shows its line or
/// card; the controls are in Setup, unless the picture would cover them.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Helper {
    /// The monitor showing the line or card.
    pub monitor: MonitorKey,
    pub method: Method,
    /// The card frame's width on the screen, in pixels.
    pub card_px: f64,
    /// The narrowest and widest the card frame may be.
    pub card_bounds: (f64, f64),
    /// The length typed from the tape, and the unit it was read in.
    pub tape_text: String,
    pub tape_unit: Unit,
    /// The picture would cover the wizard's window (a single screen), so it
    /// is shown on request, over the window.
    pub covers: bool,
}

impl Helper {
    fn mode(&self, monitor: &MonitorKey) -> PatternMode {
        match self.method {
            Method::Tape => PatternMode::Ruler {
                line_px: reference::tape_line_px(monitor.width),
                covers_wizard: self.covers,
            },
            Method::Card => PatternMode::Card {
                width_px: self.card_px,
                covers_wizard: self.covers,
            },
        }
    }
}

impl Wizard {
    pub(crate) fn reference(self) -> Option<Reference> {
        let key = self.selected_monitor()?;
        self.reference
            .read()
            .as_ref()
            .filter(|(monitor, _)| *monitor == key)
            .map(|(_, reference)| *reference)
    }

    /// The whole picture's size from the hand measurement.
    pub(super) fn reference_size(self) -> Option<KnownSize> {
        let key = self.selected_monitor()?;
        Some(self.reference()?.known_size(key.width, key.height))
    }

    /// Width and height in meters that a flat screen reports for itself, when
    /// it reported a usable size. A projector's own report says nothing true
    /// about the picture.
    pub(crate) fn panel_size_m(self) -> Option<[f64; 2]> {
        if self.kind() != DisplayKind::Flat {
            return None;
        }
        match self.finding.read().as_ref()?.known_size()? {
            KnownSize::Dimensions { width_m, height_m } => Some([width_m, height_m]),
            KnownSize::Diagonal { .. } => None,
        }
    }

    /// The width the display reports for itself, which the card starts near.
    fn panel_width_mm(self) -> Option<f64> {
        self.panel_size_m().map(|[width_m, _]| width_m * 1000.0)
    }

    /// Opens the helper. On a second screen its line or card goes up at once and
    /// the controls stay in Setup. If the picture would cover this window, as on a
    /// single screen, it is shown on request instead (`show_helper_picture`), over
    /// this window.
    pub(crate) fn open_helper(mut self, method: Method) {
        *self.helper_problem.write() = None;
        let Some(key) = self.selected_monitor() else {
            *self.helper_problem.write() =
                Some("Choose the screen you're measuring in step 2 first.".into());
            return;
        };
        let window = dioxus::desktop::window();
        let current = window.current_monitor().map(|m| monitor_key(&m));
        let covers = pattern_window::covers_window(&window, &key, current);
        let projected = self.kind() == DisplayKind::Projector;
        let start = reference::card_start_px(self.panel_width_mm(), key.width, projected);
        let card_bounds = reference::card_bounds_px(start, key.width, key.height);
        let helper = Helper {
            monitor: key.clone(),
            method,
            card_px: start.clamp(card_bounds.0, card_bounds.1),
            card_bounds,
            tape_text: String::new(),
            tape_unit: Unit::Inches,
            covers,
        };
        self.close_pattern();
        if !covers && !self.open_pattern(helper.mode(&key)) {
            *self.helper_problem.write() =
                Some("The screen you chose is no longer available.".into());
            return;
        }
        *self.helper.write() = Some(helper);
    }

    /// Single screen: puts the helper's picture up over this window. The window
    /// is not minimized: a minimized window's page stops running, and the keys
    /// that resize the frame and the close that brings it back both arrive here.
    /// Closing the picture (Escape or Enter) shows the window again.
    pub(crate) fn show_helper_picture(mut self) {
        let (Some(key), Some(helper)) = (self.selected_monitor(), self.helper.peek().clone())
        else {
            return;
        };
        *self.helper_problem.write() = None;
        if !self.open_pattern(helper.mode(&key)) {
            *self.helper_problem.write() =
                Some("The screen you chose is no longer available.".into());
        }
    }

    /// The card frame, resized from the keyboard on the picture itself.
    pub(super) fn nudge_card(self, steps: i32) {
        let px = match self.helper.peek().as_ref() {
            Some(helper) if helper.method == Method::Card => helper.card_px,
            _ => return,
        };
        self.set_card_px(px + f64::from(steps) * reference::CARD_STEP_PX);
    }

    /// Switches the open helper between the tape and the card.
    pub(crate) fn set_helper_method(mut self, method: Method) {
        let Some(key) = self.selected_monitor() else {
            return;
        };
        let mode = {
            let mut helper = self.helper.write();
            let Some(helper) = helper.as_mut() else {
                return;
            };
            helper.method = method;
            helper.mode(&key)
        };
        self.show(mode);
    }

    /// Resizes the card frame, within its bounds.
    pub(crate) fn set_card_px(mut self, px: f64) {
        let Some(key) = self.selected_monitor() else {
            return;
        };
        let mode = {
            let mut helper = self.helper.write();
            let Some(helper) = helper.as_mut() else {
                return;
            };
            let (lo, hi) = helper.card_bounds;
            helper.card_px = reference::snap(px).clamp(lo, hi);
            helper.mode(&key)
        };
        self.show(mode);
    }

    /// Records what was read off the tape.
    pub(crate) fn set_tape(mut self, text: String, unit: Unit) {
        if let Some(helper) = self.helper.write().as_mut() {
            helper.tape_text = text;
            helper.tape_unit = unit;
        }
    }

    /// What the helper gives right now: nothing when it is closed.
    pub(crate) fn helper_reading(self) -> Option<Result<Reference, ReferenceError>> {
        let key = self.selected_monitor()?;
        let helper = self.helper.read();
        let helper = helper.as_ref()?;
        Some(match helper.method {
            Method::Card => Reference::from_card(helper.card_px),
            Method::Tape => reference::parse_length_mm(&helper.tape_text, helper.tape_unit)
                .and_then(|mm| {
                    Reference::from_tape(f64::from(reference::tape_line_px(key.width)), mm)
                }),
        })
    }

    /// Takes the helper's reading as the screen's size.
    pub(crate) fn apply_helper(mut self) {
        let (Some(Ok(reading)), Some(key)) = (self.helper_reading(), self.selected_monitor())
        else {
            return;
        };
        *self.reference.write() = Some((key, reading));
        // A hand measurement replaces anything typed before it.
        self.size_text.write().clear();
        *self.manual_size.write() = false;
        self.cancel_helper();
    }

    /// Closes the helper and takes its picture down.
    pub(crate) fn cancel_helper(mut self) {
        if self.helper.peek().is_none() {
            return;
        }
        *self.helper.write() = None;
        self.close_pattern();
    }

    pub(crate) fn clear_reference(mut self) {
        *self.reference.write() = None;
    }

    /// From the review, when the camera and the declared size disagree: back
    /// to Setup with the tape ready, to settle which one is right.
    pub(crate) fn check_by_hand(self) {
        self.back_to_setup();
        self.open_helper(Method::Tape);
    }
}
