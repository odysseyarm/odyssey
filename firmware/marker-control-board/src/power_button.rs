/// Since: Odyssey Desktop 0.7.4.
pub const ENROLLMENT_HOLD_MS: u64 = 3000;

/// Hold that clears every bond; well past [`ENROLLMENT_HOLD_MS`] so a wipe is never accidental.
///
/// Since: Unreleased.
pub const CLEAR_HOLD_MS: u64 = 10_000;

/// Since: Odyssey Desktop 0.7.4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonAction {
    /// Since: Odyssey Desktop 0.7.4.
    Click,
    /// Since: Odyssey Desktop 0.7.4.
    Enroll,
    /// Since: Unreleased.
    Clear,
}

/// Since: Odyssey Desktop 0.7.4.
#[derive(Debug, Default)]
pub struct PowerButton {
    armed: bool,
    pressed_at: Option<u64>,
    long_press: bool,
    cleared: bool,
}

impl PowerButton {
    /// True from the enrollment threshold until the clear hold fires or the button is released.
    ///
    /// Since: Unreleased.
    pub fn clear_pending(&self) -> bool {
        self.long_press && !self.cleared && self.pressed_at.is_some()
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn observe(&mut self, is_low: bool, now: u64) -> Option<ButtonAction> {
        if is_low {
            if !self.armed {
                return None;
            }
            let start = *self.pressed_at.get_or_insert(now);
            if !self.cleared && now.saturating_sub(start) >= CLEAR_HOLD_MS {
                self.long_press = true;
                self.cleared = true;
                return Some(ButtonAction::Clear);
            }
            if !self.long_press && now.saturating_sub(start) >= ENROLLMENT_HOLD_MS {
                self.long_press = true;
                return Some(ButtonAction::Enroll);
            }
        } else {
            self.armed = true;
            let clicked = self.pressed_at.take().is_some() && !self.long_press;
            self.long_press = false;
            self.cleared = false;
            if clicked {
                return Some(ButtonAction::Click);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initially_held_button_cannot_enroll_or_click() {
        let mut button = PowerButton::default();
        assert_eq!(button.observe(true, 0), None);
        assert_eq!(button.observe(true, 60_000), None);
        assert_eq!(button.observe(false, 60_001), None);
        assert_eq!(button.observe(true, 60_010), None);
        assert_eq!(button.observe(false, 60_100), Some(ButtonAction::Click));
        assert_eq!(button.observe(false, 60_101), None);
    }

    #[test]
    fn long_press_opens_once_and_release_does_not_cycle_mode() {
        let mut button = PowerButton::default();
        button.observe(false, 0);
        button.observe(true, 1);
        assert_eq!(button.observe(true, 3000), None);
        assert_eq!(button.observe(true, 3001), Some(ButtonAction::Enroll));
        assert_eq!(button.observe(true, 9_000), None);
        assert_eq!(button.observe(false, 9_001), None);
        button.observe(true, 9_010);
        assert_eq!(button.observe(false, 9_100), Some(ButtonAction::Click));
    }

    #[test]
    fn clear_hold_follows_enroll_fires_once_and_release_is_silent() {
        let mut button = PowerButton::default();
        button.observe(false, 0);
        button.observe(true, 1);
        assert!(!button.clear_pending());
        assert_eq!(button.observe(true, 3001), Some(ButtonAction::Enroll));
        assert!(button.clear_pending());
        assert_eq!(button.observe(true, 10_000), None);
        assert!(button.clear_pending());
        assert_eq!(button.observe(true, 10_001), Some(ButtonAction::Clear));
        assert!(!button.clear_pending());
        assert_eq!(button.observe(true, 99_000), None);
        assert_eq!(button.observe(false, 99_001), None);
        assert!(!button.clear_pending());
        button.observe(true, 99_010);
        assert_eq!(button.observe(false, 99_100), Some(ButtonAction::Click));
    }

    #[test]
    fn early_release_never_clears_and_next_hold_can_clear() {
        let mut button = PowerButton::default();
        button.observe(false, 0);
        button.observe(true, 1);
        button.observe(true, 3001);
        assert_eq!(button.observe(false, 9_000), None);
        assert!(!button.clear_pending());
        button.observe(true, 9_100);
        assert_eq!(button.observe(true, 19_100), Some(ButtonAction::Clear));
    }
}
