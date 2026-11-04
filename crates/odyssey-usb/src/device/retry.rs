use std::future::Future;
use std::time::Duration;

/// Retry an asynchronous operation up to `limit` times.
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
