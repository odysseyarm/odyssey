use odyssey_protocol::marker_control::{LEASE_MILLIS, Mode, WriteError, parse_demand};

/// Since: Odyssey Desktop 0.7.4.
#[derive(Debug, Default)]
pub struct OutputState {
    mode: Mode,
    deadline: Option<u64>,
}

impl OutputState {
    /// Since: Odyssey Desktop 0.7.4.
    pub const fn new() -> Self {
        Self {
            mode: Mode::Auto,
            deadline: None,
        }
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn deadline(&self) -> Option<u64> {
        self.deadline
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn expire(&mut self, now: u64) {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.deadline = None;
        }
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn on(&self) -> bool {
        self.mode == Mode::On || (self.mode == Mode::Auto && self.deadline.is_some())
    }

    /// Since: Odyssey Desktop 0.7.4.
    /// Updates demand and lease deadline from a protocol value.
    ///
    /// # Errors
    /// Returns `WriteError` when `data` is not a valid demand value.
    pub fn write_demand(&mut self, data: &[u8], now: u64) -> Result<(), WriteError> {
        let active = parse_demand(data)?;
        self.deadline = if active && self.mode == Mode::Auto {
            Some(now.saturating_add(LEASE_MILLIS))
        } else {
            None
        };
        Ok(())
    }

    /// Since: Odyssey Desktop 0.7.4.
    /// Updates mode from a protocol value.
    ///
    /// # Errors
    /// Returns `WriteError` when `data` is not a valid mode value.
    pub fn write_mode(&mut self, data: &[u8]) -> Result<(), WriteError> {
        self.set_mode(Mode::parse(data)?);
        Ok(())
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.deadline = None;
            self.mode = mode;
        }
    }

    /// Short clicks cycle Auto → On → Off → Auto. A button held at boot is ignored.
    ///
    /// Since: Odyssey Desktop 0.7.4.
    pub fn click(&mut self) {
        self.set_mode(match self.mode {
            Mode::Auto => Mode::On,
            Mode::On => Mode::Off,
            Mode::Off => Mode::Auto,
        });
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub fn disconnect(&mut self) {
        self.deadline = None;
    }

    /// Pairing wins over the output mode so a button hold is always acknowledged.
    ///
    /// Since: Odyssey Desktop 0.7.4.
    pub fn led(&self, pairing: bool) -> Led {
        if pairing {
            return Led::Blue;
        }
        match self.mode {
            Mode::On => Led::Yellow,
            Mode::Off => Led::Magenta,
            Mode::Auto if self.on() => Led::Green,
            Mode::Auto => Led::Off,
        }
    }
}

/// Status LED colour. The RGB LED is common-anode to V3 behind low-side N-FETs, driven
/// from PCAL6408A P5 (red), P6 (green), P7 (blue). Schematic 117-27606 rev B sheet 3.
/// The expander has no PWM, so colours are on/off mixes.
///
/// Since: Odyssey Desktop 0.7.4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Led {
    /// Since: Odyssey Desktop 0.7.4.
    Off,
    /// Since: Odyssey Desktop 0.7.4.
    Green,
    /// Since: Odyssey Desktop 0.7.4.
    Blue,
    /// Since: Odyssey Desktop 0.7.4.
    Yellow,
    /// Since: Odyssey Desktop 0.7.4.
    Magenta,
}

impl Led {
    /// Since: Odyssey Desktop 0.7.4.
    pub const fn bits(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Green => 0x40,
            Self::Blue => 0x80,
            Self::Yellow => 0x60,
            Self::Magenta => 0xA0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn led_shows_pairing_first_then_manual_mode_then_demand() {
        let mut state = OutputState::default();
        assert_eq!(state.led(false), Led::Off);
        state.write_demand(&[1], 0).unwrap();
        assert_eq!(state.led(false), Led::Green);
        assert_eq!(state.led(true), Led::Blue);
        state.click();
        assert_eq!(state.led(false), Led::Yellow);
        state.click();
        assert_eq!(state.led(false), Led::Magenta);
        assert_eq!(state.led(true), Led::Blue);
    }

    #[test]
    fn automatic_lease_renew_clear_and_exact_expiry() {
        let mut state = OutputState::default();
        assert_eq!(state.mode(), Mode::Auto);
        assert!(!state.on());
        state.write_demand(&[1], 100).unwrap();
        assert!(state.on());
        state.expire(6099);
        assert!(state.on());
        state.write_demand(&[1], 6100).unwrap();
        assert_eq!(state.deadline(), Some(12100));
        state.expire(12100);
        assert!(!state.on());
        assert_eq!(state.deadline(), None);
        state.write_demand(&[1], 13000).unwrap();
        state.write_demand(&[0], 13001).unwrap();
        assert!(!state.on());
        assert_eq!(state.deadline(), None);
    }

    #[test]
    fn manual_modes_override_and_never_revive_stale_demand() {
        let mut state = OutputState::new();
        state.write_demand(&[1], 0).unwrap();
        state.click();
        assert_eq!(state.mode(), Mode::On);
        assert_eq!(state.deadline(), None);
        state.expire(9000);
        assert!(state.on());
        state.click();
        assert_eq!(state.mode(), Mode::Off);
        state.write_demand(&[1], 10000).unwrap();
        assert!(!state.on());
        state.click();
        assert_eq!(state.mode(), Mode::Auto);
        assert!(!state.on());
        state.write_demand(&[1], 11000).unwrap();
        state.write_mode(&[1]).unwrap();
        state.write_mode(&[0]).unwrap();
        assert!(!state.on());
        state.write_mode(&[2]).unwrap();
        state.disconnect();
        assert!(state.on());
        state.write_mode(&[0]).unwrap();
        assert!(!state.on());
    }

    #[test]
    fn disconnect_clears_lease_until_fresh_demand() {
        let mut state = OutputState::new();
        state.write_demand(&[1], 0).unwrap();
        state.disconnect();
        assert!(!state.on());
        assert_eq!(state.deadline(), None);
        state.write_mode(&[0]).unwrap();
        assert!(!state.on());
        state.write_demand(&[1], 5).unwrap();
        assert!(state.on());
    }

    #[test]
    fn invalid_writes_leave_mode_and_lease_unchanged() {
        let mut state = OutputState::new();
        state.write_demand(&[1], 100).unwrap();
        for bytes in [&[][..], &[0, 1], &[2], &[255]] {
            assert!(state.write_demand(bytes, 200).is_err());
            assert_eq!(state.deadline(), Some(6100));
            assert!(state.on());
        }
        for bytes in [&[][..], &[0, 1], &[3], &[255]] {
            assert!(state.write_mode(bytes).is_err());
            assert_eq!(state.mode(), Mode::Auto);
            assert_eq!(state.deadline(), Some(6100));
        }
    }
}
