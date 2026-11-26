//! Rate-limited delivery of marker hints to a unit.
//!
//! A hint lists the report-grid positions the tracker matched to the marker pattern. lite1
//! firmware uses it to restrict its exposure controller to real markers (see
//! `odyssey_cv::exposure` for the whole loop). `tracking.rs` calls [`MarkerHintSender::send`]
//! for every marker report. The sender stays silent unless the tracker matched enough markers,
//! so hints flow only while tracking; the firmware drops a hint after about 300 ms and falls
//! back to shape heuristics. The handler drops hints for units whose protocol predates the
//! `MarkerHint` packet.

use std::time::{Duration, Instant};

use odyssey_protocol::marker_hint::MarkerHint;
use tokio::sync::mpsc;

use super::messages::DeviceTaskMessage;

/// The firmware only needs the hint to be fresher than its own expiry, and every hint costs a
/// packet on a link that also carries the sensor streams.
const MIN_INTERVAL: Duration = Duration::from_millis(100);

/// Fewer matched markers than a pose solve needs are not worth steering exposure by.
pub(super) const MIN_POINTS: usize = 4;

/// Sends hints through the unit handler's own control channel, so the write never runs on
/// the tracking path.
#[derive(Debug, Default)]
pub(super) struct MarkerHintSender {
    sink: Option<mpsc::WeakSender<DeviceTaskMessage>>,
    last_attempt: Option<Instant>,
}

impl MarkerHintSender {
    /// The sink is weak so the tracker does not keep the handler's channel open.
    pub(super) fn set_sink(&mut self, sink: mpsc::WeakSender<DeviceTaskMessage>) {
        self.sink = Some(sink);
    }

    /// Queue a hint for `points` (report coordinates of the markers matched to the pattern).
    /// Returns whether one was queued. A full control queue drops the hint: it is stale by
    /// the time there is room.
    pub(super) fn send(&mut self, now: Instant, points: &[[u16; 2]]) -> bool {
        if points.len() < MIN_POINTS {
            return false;
        }
        let Some(sink) = &self.sink else {
            return false;
        };
        if self
            .last_attempt
            .is_some_and(|last| now.saturating_duration_since(last) < MIN_INTERVAL)
        {
            return false;
        }
        let Some(sender) = sink.upgrade() else {
            return false;
        };
        self.last_attempt = Some(now);
        let hint = MarkerHint::new(points);
        sender.try_send(DeviceTaskMessage::MarkerHint(hint)).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POINTS: [[u16; 2]; 4] = [[100, 200], [300, 400], [500, 600], [700, 800]];

    fn sender() -> (
        MarkerHintSender,
        mpsc::Sender<DeviceTaskMessage>,
        mpsc::Receiver<DeviceTaskMessage>,
    ) {
        let (tx, rx) = mpsc::channel(64);
        let mut sender = MarkerHintSender::default();
        sender.set_sink(tx.downgrade());
        (sender, tx, rx)
    }

    #[test]
    fn sends_the_hint_as_a_marker_hint_message() {
        let (mut sender, _tx, mut rx) = sender();
        assert!(sender.send(Instant::now(), &POINTS));
        let Ok(DeviceTaskMessage::MarkerHint(hint)) = rx.try_recv() else {
            panic!("expected a marker hint");
        };
        hint.validate().expect("canonical hint");
        assert_eq!(hint.points(), POINTS);
    }

    #[test]
    fn never_exceeds_ten_hertz() {
        let (mut sender, _tx, mut rx) = sender();
        let start = Instant::now();
        let mut sent = 0;
        for ms in (0..1000).step_by(5) {
            sent += usize::from(sender.send(start + Duration::from_millis(ms), &POINTS));
        }
        assert_eq!(sent, 10);
        let mut queued = 0;
        while rx.try_recv().is_ok() {
            queued += 1;
        }
        assert_eq!(queued, sent);
    }

    #[test]
    fn spacing_is_measured_from_the_last_hint() {
        let (mut sender, _tx, _rx) = sender();
        let start = Instant::now();
        assert!(sender.send(start, &POINTS));
        assert!(!sender.send(start + Duration::from_millis(99), &POINTS));
        assert!(sender.send(start + Duration::from_millis(100), &POINTS));
    }

    #[test]
    fn too_few_points_send_nothing_and_do_not_start_the_interval() {
        let (mut sender, _tx, mut rx) = sender();
        let start = Instant::now();
        assert!(!sender.send(start, &POINTS[..MIN_POINTS - 1]));
        rx.try_recv().unwrap_err();
        assert!(sender.send(start, &POINTS));
    }

    #[test]
    fn without_a_live_handler_nothing_is_sent() {
        let mut detached = MarkerHintSender::default();
        assert!(!detached.send(Instant::now(), &POINTS));

        let (mut sender, tx, _rx) = sender();
        drop(tx);
        assert!(!sender.send(Instant::now(), &POINTS));
    }

    #[test]
    fn a_full_control_queue_drops_the_hint() {
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send(DeviceTaskMessage::ResetZero)
            .expect("room for one message");
        let mut sender = MarkerHintSender::default();
        sender.set_sink(tx.downgrade());
        assert!(!sender.send(Instant::now(), &POINTS));
    }
}
