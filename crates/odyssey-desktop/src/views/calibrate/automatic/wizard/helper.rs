//! The tape and card helper: a line or card the tech measures by hand.

use gpui::Context;
use odyssey_autocalib::session::KnownSize;
use odyssey_hub_common::config::MonitorKey;

use super::Wizard;
use crate::views::calibrate::automatic::kind::DisplayKind;
use crate::views::calibrate::automatic::pattern_window::PatternMode;
use crate::views::calibrate::automatic::reference::{
    self, Method, Reference, ReferenceError, Unit,
};

/// The tape and card helper while it is open. The monitor shows its line or
/// card. The controls are in Setup, unless the picture would cover them.
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
    pub(crate) fn reference(&self) -> Option<Reference> {
        let key = self.selected.as_ref()?;
        self.reference
            .as_ref()
            .filter(|(monitor, _)| monitor == key)
            .map(|(_, reference)| *reference)
    }

    /// The whole picture's size from the hand measurement.
    pub(super) fn reference_size(&self) -> Option<KnownSize> {
        let key = self.selected.as_ref()?;
        Some(self.reference()?.known_size(key.width, key.height))
    }

    /// Width and height in meters that a flat screen reports for itself, when
    /// it reported a usable size. A projector's own report says nothing true
    /// about the picture.
    pub(crate) fn panel_size_m(&self) -> Option<[f64; 2]> {
        if self.kind() != DisplayKind::Flat {
            return None;
        }
        match self.finding.as_ref()?.known_size()? {
            KnownSize::Dimensions { width_m, height_m } => Some([width_m, height_m]),
            KnownSize::Diagonal { .. } => None,
        }
    }

    /// The width the display reports for itself, which the card starts near.
    fn panel_width_mm(&self) -> Option<f64> {
        self.panel_size_m().map(|[width_m, _]| width_m * 1000.0)
    }

    /// Opens the helper. On a second screen its line or card goes up at once and
    /// the controls stay in Setup. If the picture would cover this window, as on a
    /// single screen, it is shown on request instead (`show_helper_picture`), over
    /// this window.
    pub(crate) fn open_helper(&mut self, method: Method, cx: &mut Context<Self>) {
        self.helper_problem = None;
        let Some(key) = self.selected.clone() else {
            self.helper_problem =
                Some("Choose the screen you're measuring in step 2 first.".into());
            cx.notify();
            return;
        };
        let covers = self.pattern_covers_host(cx);
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
        self.close_pattern(cx);
        if !covers && !self.open_pattern(helper.mode(&key), cx) {
            self.helper_problem = Some("The screen you chose is no longer available.".into());
            cx.notify();
            return;
        }
        self.helper = Some(helper);
        cx.notify();
    }

    /// Single screen: puts the helper's picture up over this window. The window
    /// is not minimized, so the keys that resize the frame and the close that
    /// brings it back still reach this wizard. Closing the picture (Escape or
    /// Enter) shows the window again.
    pub(crate) fn show_helper_picture(&mut self, cx: &mut Context<Self>) {
        let (Some(key), Some(helper)) = (self.selected.clone(), self.helper.clone()) else {
            return;
        };
        self.helper_problem = None;
        if !self.open_pattern(helper.mode(&key), cx) {
            self.helper_problem = Some("The screen you chose is no longer available.".into());
        }
        cx.notify();
    }

    /// The card frame, resized from the keyboard on the picture itself.
    pub(super) fn nudge_card(&mut self, steps: i32, cx: &mut Context<Self>) {
        let px = match self.helper.as_ref() {
            Some(helper) if helper.method == Method::Card => helper.card_px,
            _ => return,
        };
        self.set_card_px(px + f64::from(steps) * reference::CARD_STEP_PX, cx);
    }

    /// Switches the open helper between the tape and the card.
    pub(crate) fn set_helper_method(&mut self, method: Method, cx: &mut Context<Self>) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        let Some(helper) = self.helper.as_mut() else {
            return;
        };
        helper.method = method;
        let mode = helper.mode(&key);
        self.show(mode, cx);
        cx.notify();
    }

    /// Resizes the card frame, within its bounds.
    pub(crate) fn set_card_px(&mut self, px: f64, cx: &mut Context<Self>) {
        let Some(key) = self.selected.clone() else {
            return;
        };
        let Some(helper) = self.helper.as_mut() else {
            return;
        };
        let (lo, hi) = helper.card_bounds;
        helper.card_px = reference::snap(px).clamp(lo, hi);
        let mode = helper.mode(&key);
        self.show(mode, cx);
        cx.notify();
    }

    /// Records what was read off the tape.
    pub(crate) fn set_tape(&mut self, text: String, unit: Unit, cx: &mut Context<Self>) {
        if let Some(helper) = self.helper.as_mut() {
            helper.tape_text = text;
            helper.tape_unit = unit;
            cx.notify();
        }
    }

    /// What the helper gives right now: nothing when it is closed.
    pub(crate) fn helper_reading(&self) -> Option<Result<Reference, ReferenceError>> {
        let key = self.selected.as_ref()?;
        let helper = self.helper.as_ref()?;
        Some(match helper.method {
            Method::Card => Reference::from_card(helper.card_px),
            Method::Tape => reference::parse_length_mm(&helper.tape_text, helper.tape_unit)
                .and_then(|mm| {
                    Reference::from_tape(f64::from(reference::tape_line_px(key.width)), mm)
                }),
        })
    }

    /// Takes the helper's reading as the screen's size.
    pub(crate) fn apply_helper(&mut self, cx: &mut Context<Self>) {
        let (Some(Ok(reading)), Some(key)) = (self.helper_reading(), self.selected.clone()) else {
            return;
        };
        self.reference = Some((key, reading));
        // A hand measurement replaces anything typed before it.
        self.size_text.clear();
        self.manual_size = false;
        self.cancel_helper(cx);
        cx.notify();
    }

    /// Closes the helper and takes its picture down.
    pub(crate) fn cancel_helper(&mut self, cx: &mut Context<Self>) {
        if self.helper.take().is_none() {
            return;
        }
        self.close_pattern(cx);
        cx.notify();
    }

    pub(crate) fn clear_reference(&mut self, cx: &mut Context<Self>) {
        self.reference = None;
        cx.notify();
    }

    /// From the review, when the camera and the declared size disagree: back
    /// to Setup with the tape ready, to settle which one is right.
    pub(crate) fn check_by_hand(&mut self, cx: &mut Context<Self>) {
        self.back_to_setup(cx);
        self.open_helper(Method::Tape, cx);
    }
}
