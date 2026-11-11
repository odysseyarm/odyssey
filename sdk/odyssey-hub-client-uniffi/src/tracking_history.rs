use std::sync::Arc;

use parking_lot::Mutex;

use crate::ffi_common::TrackingEvent;

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Object)]
pub struct TrackingHistory {
    inner: Arc<Mutex<odyssey_hub_client::tracking_history::TrackingHistory>>,
}

impl std::fmt::Debug for TrackingHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackingHistory").finish_non_exhaustive()
    }
}

#[uniffi::export]
impl TrackingHistory {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    #[uniffi::constructor]
    pub fn new(capacity: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(
                odyssey_hub_client::tracking_history::TrackingHistory::new(capacity as usize),
            )),
        }
    }

    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    #[uniffi::method]
    pub fn push(&self, event: TrackingEvent) {
        self.inner.lock().push(event.into());
    }

    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    #[uniffi::method]
    pub fn latest(&self) -> Option<TrackingEvent> {
        self.inner.lock().latest().map(Into::into)
    }

    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    #[uniffi::method]
    pub fn get_closest(&self, timestamp: u32) -> Option<TrackingEvent> {
        self.inner.lock().get_closest(timestamp).map(Into::into)
    }
}
