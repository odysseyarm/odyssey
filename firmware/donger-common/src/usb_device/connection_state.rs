/// Kept in the USB watch so a reset followed by reconfiguration cannot disappear
/// between polls. Generations identify connection boundaries, not host sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ConnectionState {
    pub(super) active: bool,
    pub(super) generation: u32,
}

impl ConnectionState {
    pub(super) const INITIAL: Self = Self {
        active: false,
        generation: 0,
    };

    pub(super) fn set_active(&mut self, active: bool) -> bool {
        if self.active == active {
            return false;
        }
        if !active {
            self.generation = self.generation.wrapping_add(1);
        }
        self.active = active;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::ConnectionState;

    #[test]
    fn brief_disconnect_remains_observable_after_reconfiguration() {
        let mut state = ConnectionState::INITIAL;
        state.set_active(true);
        let original = state;
        state.set_active(false);
        state.set_active(true);
        assert!(state.active);
        assert_ne!(state.generation, original.generation);
        assert_eq!(state.generation, original.generation.wrapping_add(1));
    }

    #[test]
    fn duplicate_events_do_not_invalidate_the_connection() {
        let mut state = ConnectionState::INITIAL;
        state.set_active(true);
        let original = state;
        assert!(!state.set_active(true));
        assert_eq!(state, original);
        state.set_active(false);
        let disconnected = state;
        assert!(!state.set_active(false));
        assert_eq!(state, disconnected);
    }

    #[test]
    fn connection_epoch_wraps_without_panicking() {
        let mut state = ConnectionState {
            active: true,
            generation: u32::MAX,
        };
        state.set_active(false);
        assert_eq!(state.generation, 0);
        assert!(!state.active);
    }
}
