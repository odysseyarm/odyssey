use std::time::Duration;

use backon::ConstantBuilder;

/// Per-attempt timeout when a stream-control packet is retried: the first
/// packet after a device (re)boots is sometimes lost, so these are re-sent.
pub(super) const STREAM_CONTROL_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(2000);
/// How many times a stream-control packet is sent before giving up.
pub(super) const STREAM_CONTROL_ATTEMPTS: usize = 5;

/// Schedule for re-sending after a silent attempt: `limit` attempts in total,
/// re-sent immediately because the attempt's own timeout is the wait.
pub(super) fn resend_schedule(limit: usize) -> ConstantBuilder {
    ConstantBuilder::default()
        .with_delay(Duration::ZERO)
        .with_max_times(limit.saturating_sub(1))
}
