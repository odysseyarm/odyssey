use std::sync::Arc;

use parking_lot::Mutex;

use crate::ffi_common::TrackingEvent;

#[derive(uniffi::Object)]
pub struct TrackingHistory {
    inner: Arc<Mutex<odyssey_hub_client::tracking_history::TrackingHistory>>,
}

#[uniffi::export]
impl TrackingHistory {
    #[uniffi::constructor]
    pub fn new(capacity: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(
                odyssey_hub_client::tracking_history::TrackingHistory::new(capacity as usize),
            )),
        }
    }

    #[uniffi::method]
    pub fn push(&self, event: TrackingEvent) {
        self.inner.lock().push(event.into());
    }

    #[uniffi::method]
    pub fn latest(&self) -> Option<TrackingEvent> {
        self.inner.lock().latest().map(|e| e.into())
    }

    #[uniffi::method]
    pub fn get_closest(&self, timestamp: u32) -> Option<TrackingEvent> {
        self.inner.lock().get_closest(timestamp).map(|e| e.into())
    }
}
