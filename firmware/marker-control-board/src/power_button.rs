/// Toggles on complete debounced clicks; an initially held button must first be released.
#[derive(Default)]
pub struct PowerButton {
    armed: bool,
    pressed: bool,
    on: bool,
}

impl PowerButton {
    pub fn woken_by_button() -> Self {
        Self {
            armed: true,
            pressed: true,
            on: false,
        }
    }

    pub fn observe(&mut self, is_low: bool) -> Option<bool> {
        if is_low {
            self.pressed = self.armed;
        } else {
            self.armed = true;
            if self.pressed {
                self.pressed = false;
                self.on = !self.on;
                return Some(self.on);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initially_held_button_and_repeated_levels_do_not_toggle() {
        let mut button = PowerButton::default();
        assert_eq!(button.observe(true), None);
        assert_eq!(button.observe(false), None);
        assert_eq!(button.observe(false), None);
        assert_eq!(button.observe(true), None);
        assert_eq!(button.observe(false), Some(true));
        assert_eq!(button.observe(true), None);
        for _ in 0..100 {
            assert_eq!(button.observe(true), None);
        }
        assert_eq!(button.observe(false), Some(false));
    }

    #[test]
    fn system_off_wake_turns_on_only_after_button_release() {
        let mut button = PowerButton::woken_by_button();
        assert_eq!(button.observe(true), None);
        assert_eq!(button.observe(false), Some(true));
        assert_eq!(button.observe(false), None);
        assert_eq!(button.observe(true), None);
        assert_eq!(button.observe(false), Some(false));
    }
}
