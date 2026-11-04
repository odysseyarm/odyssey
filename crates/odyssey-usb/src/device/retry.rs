use std::time::Duration;

/// Per-attempt timeout when a stream-control packet is retried: the first
/// packet after a device (re)boots is sometimes lost, so these are re-sent.
pub(super) const STREAM_CONTROL_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(2000);
/// How many times a stream-control packet is sent before giving up.
pub(super) const STREAM_CONTROL_ATTEMPTS: usize = 5;

/// Retry an asynchronous operation up to `limit` times, each bounded by
/// `timeout`. `None` when every attempt timed out.
pub(super) async fn retry<F, G>(mut op: F, timeout: Duration, limit: usize) -> Option<G::Output>
where
    F: FnMut() -> G,
    G: Future,
{
    for _ in 0..limit {
        if let Ok(r) = tokio::time::timeout(timeout, op()).await {
            return Some(r);
        }
    }
    None
}
