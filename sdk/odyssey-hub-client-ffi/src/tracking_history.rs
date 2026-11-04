use std::sync::Arc;

use odyssey_hub_client::tracking_history::TrackingHistory as TrackingHistoryInner;
use parking_lot::Mutex;

use crate::event::TrackingEvent;

pub struct TrackingHistory {
    inner: Arc<Mutex<TrackingHistoryInner>>,
}

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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tracking_history_push(
    history: *mut TrackingHistory,
    event: TrackingEvent,
) {
    if let Some(history) = unsafe { history.as_ref() } {
        let mut lock = history.inner.lock();
        let common_event: odyssey_hub_common::events::TrackingEvent = event.into();
        lock.push(common_event.into());
    }
}

/// # Safety
/// `history` must be null or live. A non-null `out_event` must be aligned and
/// writable for one event. Null output pointers return false.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tracking_history_latest(
    history: *mut TrackingHistory,
    out_event: *mut TrackingEvent,
) -> bool {
    if out_event.is_null() {
        return false;
    }
    if let Some(history) = unsafe { history.as_ref() } {
        let lock = history.inner.lock();
        if let Some(e) = lock.latest() {
            let common_event: odyssey_hub_common::events::TrackingEvent = e.into();
            unsafe {
                *out_event = common_event.into();
            }
            return true;
        }
    }
    false
}

/// # Safety
/// `history` must be null or live. A non-null `out_event` must be aligned and
/// writable for one event. Null output pointers return false.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tracking_history_get_closest(
    history: *mut TrackingHistory,
    timestamp: u32,
    out_event: *mut TrackingEvent,
) -> bool {
    if out_event.is_null() {
        return false;
    }
    if let Some(history) = unsafe { history.as_ref() } {
        let lock = history.inner.lock();
        if let Some(e) = lock.get_closest(timestamp) {
            let common_event: odyssey_hub_common::events::TrackingEvent = e.into();
            unsafe {
                *out_event = common_event.into();
            }
            return true;
        }
    }
    false
}

/// # Safety
/// `history` must be null or an owned, unborrowed handle from `tracking_history_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tracking_history_free(history: *mut TrackingHistory) {
    if !history.is_null() {
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
            aimpoint: Default::default(),
            pose: Default::default(),
            distance: 1.0,
            screen_id: 0,
        };
        let mut output = std::mem::MaybeUninit::uninit();
        unsafe {
            tracking_history_push(history, event);
            assert!(!tracking_history_latest(history, std::ptr::null_mut()));
            assert!(!tracking_history_get_closest(
                history,
                42,
                std::ptr::null_mut()
            ));
            assert!(tracking_history_latest(history, output.as_mut_ptr()));
            assert_eq!(output.assume_init().timestamp, 42);
            tracking_history_free(history);
        }
    }
}
