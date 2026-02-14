use crate::event::TrackingEvent;
use odyssey_hub_client::tracking_history::TrackingHistory as TrackingHistoryInner;
use std::sync::{Arc, Mutex};

pub struct TrackingHistory {
    inner: Arc<Mutex<TrackingHistoryInner>>,
}

#[no_mangle]
pub extern "C" fn tracking_history_new(capacity: u32) -> *mut TrackingHistory {
    let inner = TrackingHistoryInner::new(capacity as usize);
    let history = TrackingHistory {
        inner: Arc::new(Mutex::new(inner)),
    };
    Box::into_raw(Box::new(history))
}

#[no_mangle]
pub extern "C" fn tracking_history_push(history: *mut TrackingHistory, event: TrackingEvent) {
    if let Some(history) = unsafe { history.as_ref() } {
        let mut lock = history.inner.lock().unwrap();
        let common_event: odyssey_hub_common::events::TrackingEvent = event.into();
        lock.push(common_event.into());
    }
}

#[no_mangle]
pub extern "C" fn tracking_history_get_closest(
    history: *mut TrackingHistory,
    timestamp: u32,
    out_event: *mut TrackingEvent,
) -> bool {
    if let Some(history) = unsafe { history.as_ref() } {
        let lock = history.inner.lock().unwrap();
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

#[no_mangle]
pub extern "C" fn tracking_history_free(history: *mut TrackingHistory) {
    if !history.is_null() {
        unsafe {
            drop(Box::from_raw(history));
        }
    }
}
