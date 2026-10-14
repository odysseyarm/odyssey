#![allow(unused)]
use core::{
    pin::Pin,
    task::{Context, Poll, Waker},
};

use embassy_time::{Duration, Instant};

/// Copy of [`embassy_time::Timer`] that tolerates noop wakers
pub struct Timer {
    expires_at: Instant,
    yielded_once: bool,
}
impl Timer {
    /// Expire at specified [Instant](struct.Instant.html)
    /// Will expire immediately if the Instant is in the past.
    pub fn at(expires_at: Instant) -> Self {
        Self {
            expires_at,
            yielded_once: false,
        }
    }

    /// Expire after specified [Duration](struct.Duration.html).
    /// This can be used as a `sleep` abstraction.
    ///
    /// Example:
    /// ``` no_run
    /// use embassy_time::{Duration, Timer};
    ///
    /// #[embassy_executor::task]
    /// async fn demo_sleep_seconds() {
    ///     // suspend this task for one second.
    ///     Timer::after(Duration::from_secs(1)).await;
    /// }
    /// ```
    pub fn after(duration: Duration) -> Self {
        Self {
            expires_at: Instant::now() + duration,
            yielded_once: false,
        }
    }

    /// Expire after the specified number of ticks.
    ///
    /// This method is a convenience wrapper for calling `Timer::after(Duration::from_ticks())`.
    /// For more details, refer to [`Timer::after()`] and [`Duration::from_ticks()`].
    #[inline]
    pub fn after_ticks(ticks: u64) -> Self {
        Self::after(Duration::from_ticks(ticks))
    }

    /// Expire after the specified number of nanoseconds.
    ///
    /// This method is a convenience wrapper for calling `Timer::after(Duration::from_nanos())`.
    /// For more details, refer to [`Timer::after()`] and [`Duration::from_nanos()`].
    #[inline]
    pub fn after_nanos(nanos: u64) -> Self {
        Self::after(Duration::from_nanos(nanos))
    }

    /// Expire after the specified number of microseconds.
    ///
    /// This method is a convenience wrapper for calling `Timer::after(Duration::from_micros())`.
    /// For more details, refer to [`Timer::after()`] and [`Duration::from_micros()`].
    #[inline]
    pub fn after_micros(micros: u64) -> Self {
        Self::after(Duration::from_micros(micros))
    }

    /// Expire after the specified number of milliseconds.
    ///
    /// This method is a convenience wrapper for calling `Timer::after(Duration::from_millis())`.
    /// For more details, refer to [`Timer::after`] and [`Duration::from_millis()`].
    #[inline]
    pub fn after_millis(millis: u64) -> Self {
        Self::after(Duration::from_millis(millis))
    }

    /// Expire after the specified number of seconds.
    ///
    /// This method is a convenience wrapper for calling `Timer::after(Duration::from_secs())`.
    /// For more details, refer to [`Timer::after`] and [`Duration::from_secs()`].
    #[inline]
    pub fn after_secs(secs: u64) -> Self {
        Self::after(Duration::from_secs(secs))
    }
}

impl Future for Timer {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.yielded_once && self.expires_at <= Instant::now() {
            return Poll::Ready(());
        }
        if cx.waker() as *const Waker != NOOP_WAKER as *const Waker {
            defmt::trace!("schedule wake");
            embassy_time_driver::schedule_wake(self.expires_at.as_ticks(), cx.waker());
        }
        self.yielded_once = true;
        Poll::Pending
    }
}

// Use this waker to make sure it has the same address
pub static NOOP_WAKER: &'static Waker = Waker::noop();
