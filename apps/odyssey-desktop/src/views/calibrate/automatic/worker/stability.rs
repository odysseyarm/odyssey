//! Timing of how long a condition has held.

use std::time::{Duration, Instant};

/// How long a condition has held without a break. The camera thread feeds it
/// every frame, so a dropped UI update cannot hide a flicker.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Stability {
    since: Option<Instant>,
}

impl Stability {
    /// Records one observation at `now` and returns how long `holds` has been
    /// true in a row.
    pub(crate) fn observe(&mut self, holds: bool, now: Instant) -> Duration {
        if !holds {
            self.since = None;
            return Duration::ZERO;
        }
        now.saturating_duration_since(*self.since.get_or_insert(now))
    }

    pub(crate) fn reset(&mut self) {
        self.since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stability_counts_an_unbroken_run_and_resets_on_a_gap() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut s = Stability::default();
        assert_eq!(s.observe(true, t0), ms(0));
        assert_eq!(s.observe(true, t0 + ms(400)), ms(400));
        assert_eq!(s.observe(true, t0 + ms(1000)), ms(1000));
        assert_eq!(s.observe(false, t0 + ms(1100)), ms(0));
        // The run starts over after the gap rather than resuming.
        assert_eq!(s.observe(true, t0 + ms(1200)), ms(0));
        assert_eq!(s.observe(true, t0 + ms(1500)), ms(300));
    }
}
