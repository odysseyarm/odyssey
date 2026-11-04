//! Pacing for the update's waits on a device that has to come back or settle.

use std::iter;
use std::time::{Duration, Instant};

/// The device is not in the wanted state yet; the poll tries again until its
/// window closes.
pub(super) struct NotYet;

/// `Ok` once `done`, so a plain check can drive a `backon` retry.
pub(super) fn ready(done: bool) -> Result<(), NotYet> {
    if done { Ok(()) } else { Err(NotYet) }
}

/// A backoff that retries every `interval` until `timeout` has passed,
/// measured from this call.
///
/// The window is wall-clock time, so a slow attempt uses it up: `backon`'s
/// own `total_delay` counts only the sleeps. The deadline is checked when an
/// attempt fails, so the last retry may start just inside the window and end
/// after it.
pub(super) fn poll_window(
    timeout: Duration,
    interval: Duration,
) -> impl Iterator<Item = Duration> + Send + Sync + Unpin {
    let deadline = Instant::now() + timeout;
    iter::repeat(interval).take_while(move |_| Instant::now() < deadline)
}
