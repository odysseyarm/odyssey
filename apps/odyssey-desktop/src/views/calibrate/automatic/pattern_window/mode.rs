//! What the pattern window shows and what it tells the wizard.

/// What the pattern window shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PatternMode {
    /// The dot pattern, white on black.
    Pattern,
    /// A plain black screen.
    Black,
    /// Seconds left before the pattern appears, over black.
    Countdown(u32),
    /// A line `line_px` pixels long with a tick at each end, for a tape
    /// measure. Drawn in screen pixels, not in the pattern's unit space.
    /// `covers_wizard` is set when this picture is covering the wizard's window, so it
    /// also says how to come back.
    Ruler { line_px: u32, covers_wizard: bool },
    /// A frame whose inside is a credit card `width_px` pixels wide, for
    /// holding a real card against. Drawn in screen pixels. `covers_wizard` as for
    /// the ruler; it also says the arrow keys resize the frame.
    Card { width_px: f64, covers_wizard: bool },
}

/// Why the pattern window can no longer do its job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PatternLoss {
    /// Someone closed the window (Escape or Alt+F4).
    Closed,
    /// The window manager put the window on a different monitor than the one
    /// being calibrated, so the camera would measure the wrong screen.
    WrongMonitor,
}

/// What the pattern window tells the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PatternEvent {
    Lost(PatternLoss),
    /// The card frame should grow (positive) or shrink by this many steps of
    /// [`crate::views::calibrate::automatic::reference::CARD_STEP_PX`].
    /// Sent from the keyboard, which resizes the frame while this window hides
    /// the wizard's own.
    Nudge(i32),
}

/// How many steps Page Up and Page Down move the card frame.
pub(super) const CARD_BIG_STEPS: i32 = 20;
