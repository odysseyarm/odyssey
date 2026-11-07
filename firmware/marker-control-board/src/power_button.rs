pub const ENROLLMENT_HOLD_MS: u64 = 3000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonAction {
    Click,
    Enroll,
}

#[derive(Default)]
pub struct PowerButton {
    armed: bool,
    pressed_at: Option<u64>,
    long_press: bool,
}

impl PowerButton {
    pub fn observe(&mut self, is_low: bool, now: u64) -> Option<ButtonAction> {
        if is_low {
            if !self.armed {
                return None;
            }
            let start = *self.pressed_at.get_or_insert(now);
            if !self.long_press && now.saturating_sub(start) >= ENROLLMENT_HOLD_MS {
                self.long_press = true;
                return Some(ButtonAction::Enroll);
            }
        } else {
            self.armed = true;
            let clicked = self.pressed_at.take().is_some() && !self.long_press;
            self.long_press = false;
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
        assert_eq!(button.observe(true, 90_000), None);
        assert_eq!(button.observe(false, 90_001), None);
        button.observe(true, 90_010);
        assert_eq!(button.observe(false, 90_100), Some(ButtonAction::Click));
    }
}
