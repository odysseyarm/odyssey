use odyssey_hub_common::events::TrackingEvent;

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

    pub fn get_closest(&self, timestamp: u32) -> Option<TrackingEvent> {
        let len = if self.full { self.capacity } else { self.head };
        let mut best: Option<TrackingEvent> = None;
        let mut best_diff = u32::MAX;

        for i in 0..len {
            if let Some(ev) = self.buffer[i] {
                let diff = ev.timestamp.abs_diff(timestamp);
                if diff < best_diff {
                    best_diff = diff;
                    best = Some(ev);
                }
            }
        }
        best
    }
}
