use odyssey_hub_common::events::TrackingEvent;

/// Ring buffer of the most recent tracking events of one device.
#[derive(Debug)]
pub struct TrackingHistory {
    buffer: Vec<Option<TrackingEvent>>,
    head: usize,
    full: bool,
    capacity: usize,
}

impl TrackingHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![None; capacity],
            head: 0,
            full: false,
            capacity,
        }
    }

    pub fn push(&mut self, event: TrackingEvent) {
        self.buffer[self.head] = Some(event);
        self.head = (self.head + 1) % self.capacity;
        if self.head == 0 {
            self.full = true;
        }
    }

    pub fn latest(&self) -> Option<TrackingEvent> {
        if self.head == 0 && !self.full {
            return None;
        }
        let idx = (self.head + self.capacity - 1) % self.capacity;
        self.buffer[idx]
    }

    /// The stored event whose timestamp is nearest to `timestamp`; the oldest
    /// one wins a tie.
    pub fn get_closest(&self, timestamp: u32) -> Option<TrackingEvent> {
        let len = if self.full { self.capacity } else { self.head };
        let mut best: Option<TrackingEvent> = None;
        let mut best_diff = u32::MAX;

        for event in self.buffer[..len].iter().flatten() {
            let diff = event.timestamp.abs_diff(timestamp);
            if diff < best_diff {
                best_diff = diff;
                best = Some(*event);
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_at(timestamp: u32) -> TrackingEvent {
        TrackingEvent {
            timestamp,
            aimpoint: Default::default(),
            pose: odyssey_hub_common::events::Pose {
                rotation: Default::default(),
                translation: Default::default(),
            },
            distance: 0.0,
            screen_id: 0,
        }
    }

    #[test]
    fn empty_history_has_nothing() {
        let history = TrackingHistory::new(3);
        assert!(history.latest().is_none());
        assert!(history.get_closest(10).is_none());
    }

    #[test]
    fn latest_follows_the_newest_push_across_wraparound() {
        let mut history = TrackingHistory::new(2);
        for timestamp in [10, 20, 30] {
            history.push(event_at(timestamp));
            assert_eq!(history.latest().map(|e| e.timestamp), Some(timestamp));
        }
    }

    #[test]
    fn closest_ignores_overwritten_events() {
        let mut history = TrackingHistory::new(2);
        for timestamp in [10, 20, 30] {
            history.push(event_at(timestamp));
        }
        // 10 was overwritten, so 20 is the nearest to 11.
        assert_eq!(history.get_closest(11).map(|e| e.timestamp), Some(20));
        assert_eq!(history.get_closest(100).map(|e| e.timestamp), Some(30));
    }

    #[test]
    fn closest_prefers_the_older_event_on_a_tie() {
        let mut history = TrackingHistory::new(4);
        history.push(event_at(10));
        history.push(event_at(20));
        assert_eq!(history.get_closest(15).map(|e| e.timestamp), Some(10));
    }
}
