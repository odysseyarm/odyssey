use std::sync::Arc;

use odyssey_hub_client::tracking_history::TrackingHistory as TrackingHistoryInner;
use parking_lot::Mutex;

use crate::event::TrackingEvent;

/// Since: FFI SDK 0.1.0 (earliest tagged release).
pub struct TrackingHistory {
    inner: Arc<Mutex<TrackingHistoryInner>>,
}

impl std::fmt::Debug for TrackingHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackingHistory").finish_non_exhaustive()
    }
}

/// Since: FFI SDK 0.1.0 (earliest tagged release).
#[unsafe(no_mangle)]
pub extern "C" fn tracking_history_new(capacity: u32) -> *mut TrackingHistory {
    let inner = TrackingHistoryInner::new(capacity as usize);
    let history = TrackingHistory {
        inner: Arc::new(Mutex::new(inner)),
    };
    Box::into_raw(Box::new(history))
}

/// # Safety
/// `history` must be null or a live history handle for the duration of the call.
///
/// # Since
/// FFI SDK 0.1.0 (earliest tagged release).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tracking_history_push(
    history: *mut TrackingHistory,
    event: TrackingEvent,
) {
    // SAFETY: This function requires a live handle or null.
    if let Some(history) = unsafe { history.as_ref() } {
        history.inner.lock().push(event.into());
    }
}

/// # Safety
/// `history` must be null or live. A non-null `out_event` must be aligned and
/// writable for one event. Null output pointers return false.
///
/// # Since
/// FFI SDK 0.1.0 (earliest tagged release).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tracking_history_latest(
    history: *mut TrackingHistory,
    out_event: *mut TrackingEvent,
) -> bool {
    if out_event.is_null() {
        return false;
    }
    // SAFETY: This function requires a live handle or null.
    if let Some(history) = unsafe { history.as_ref() } {
        let lock = history.inner.lock();
        if let Some(e) = lock.latest() {
            // SAFETY: This function requires an aligned writable event pointer.
            unsafe { *out_event = e.into() };
            return true;
        }
    }
    false
}

/// # Safety
/// `history` must be null or live. A non-null `out_event` must be aligned and
/// writable for one event. Null output pointers return false.
///
/// # Since
/// FFI SDK 0.1.0 (earliest tagged release).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tracking_history_get_closest(
    history: *mut TrackingHistory,
    timestamp: u32,
    out_event: *mut TrackingEvent,
) -> bool {
    if out_event.is_null() {
        return false;
    }
    // SAFETY: This function requires a live handle or null.
    if let Some(history) = unsafe { history.as_ref() } {
        let lock = history.inner.lock();
        if let Some(e) = lock.get_closest(timestamp) {
            // SAFETY: This function requires an aligned writable event pointer.
            unsafe { *out_event = e.into() };
            return true;
        }
    }
    false
}

/// # Safety
/// `history` must be null or an owned, unborrowed handle from `tracking_history_new`.
///
/// # Since
/// FFI SDK 0.1.0 (earliest tagged release).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tracking_history_free(history: *mut TrackingHistory) {
    if !history.is_null() {
        // SAFETY: This function requires an owned, unborrowed live handle.
        unsafe {
            drop(Box::from_raw(history));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn populated_history_rejects_null_output_without_losing_its_event() {
        let history = tracking_history_new(1);
        let event = TrackingEvent {
            timestamp: 42,
            aimpoint: crate::funny::Vector2f32::default(),
            pose: crate::event::Pose::default(),
            distance: 1.0,
            screen_id: 0,
        };
        let mut output = std::mem::MaybeUninit::uninit();
        // SAFETY: `history` is a live handle returned by `tracking_history_new`.
        unsafe { tracking_history_push(history, event) };
        // SAFETY: `history` is live. Null output is explicitly accepted.
        assert!(!unsafe { tracking_history_latest(history, std::ptr::null_mut()) });
        // SAFETY: `history` is live. Null output is explicitly accepted.
        assert!(!unsafe {
            tracking_history_get_closest(history, 42, std::ptr::null_mut())
        });
        // SAFETY: `history` is live and `output` provides aligned writable storage.
        assert!(unsafe { tracking_history_latest(history, output.as_mut_ptr()) });
        // SAFETY: A successful latest call initialized `output`.
        let output = unsafe { output.assume_init() };
        assert_eq!(output.timestamp, 42);
        // SAFETY: This test owns the live handle and no call borrows it.
        unsafe { tracking_history_free(history) };
    }
}
