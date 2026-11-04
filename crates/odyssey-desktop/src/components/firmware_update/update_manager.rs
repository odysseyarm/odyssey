use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use dioxus::prelude::{Signal, WritableExt as _, use_future, use_signal};
use parking_lot::Mutex;

use super::update_state::FirmwareUpdateState;

/// Firmware update state per device, shared between the tokio update tasks
/// and the Dioxus components that poll it.
#[derive(Clone)]
pub(crate) struct FirmwareUpdateManager {
    states: Arc<Mutex<HashMap<[u8; 6], FirmwareUpdateState>>>,
    updating: Arc<Mutex<HashSet<[u8; 6]>>>,
}

/// Keeps a device claimed until its update future and final result are gone,
/// including when that future is cancelled before it is first polled.
pub(super) struct UpdateClaim {
    manager: FirmwareUpdateManager,
    uuid: [u8; 6],
}

impl Drop for UpdateClaim {
    fn drop(&mut self) {
        self.manager.clear_state(&self.uuid);
    }
}

impl PartialEq for FirmwareUpdateManager {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.states, &other.states)
    }
}

impl Default for FirmwareUpdateManager {
    fn default() -> Self {
        Self::new()
    }
}

impl FirmwareUpdateManager {
    pub(crate) fn new() -> Self {
        Self {
            states: Arc::new(Mutex::new(HashMap::new())),
            updating: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub(super) fn get_state(&self, uuid: &[u8; 6]) -> FirmwareUpdateState {
        self.states
            .lock()
            .get(uuid)
            .cloned()
            .unwrap_or(FirmwareUpdateState::Idle)
    }

    pub(super) fn set_state(&self, uuid: [u8; 6], state: FirmwareUpdateState) {
        self.states.lock().insert(uuid, state);
    }

    /// Availability is computed during render and may already be stale when it
    /// is ready to publish. Never let it overwrite a task's progress or result.
    pub(super) fn set_before_update_state(&self, uuid: [u8; 6], state: FirmwareUpdateState) {
        let mut states = self.states.lock();
        let current = states.entry(uuid).or_insert(FirmwareUpdateState::Idle);
        if current.is_before_update() {
            *current = state;
        }
    }

    /// Claim a device for the lifetime of one update. The check and insert are
    /// atomic, so two clicks cannot start competing transfers.
    pub(super) fn try_claim(&self, uuid: [u8; 6]) -> Option<UpdateClaim> {
        if !self.updating.lock().insert(uuid) {
            return None;
        }
        self.set_state(uuid, FirmwareUpdateState::Downloading);
        Some(UpdateClaim {
            manager: self.clone(),
            uuid,
        })
    }

    pub(super) fn is_updating(&self, uuid: &[u8; 6]) -> bool {
        self.updating.lock().contains(uuid)
    }

    pub(crate) fn get_all_updating(&self) -> Vec<[u8; 6]> {
        self.updating.lock().iter().copied().collect()
    }

    fn clear_state(&self, uuid: &[u8; 6]) {
        self.states.lock().remove(uuid);
        self.updating.lock().remove(uuid);
    }
}

/// How often components re-read the manager, which the update task writes from
/// another thread.
const STATE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A signal that follows `uuid`'s firmware update state in `manager`.
pub(super) fn use_polled_state(
    manager: &FirmwareUpdateManager,
    uuid: [u8; 6],
) -> Signal<FirmwareUpdateState> {
    let mut state = use_signal(|| manager.get_state(&uuid));
    let manager = manager.clone();
    use_future(move || {
        let manager = manager.clone();
        async move {
            loop {
                tokio::time::sleep(STATE_POLL_INTERVAL).await;
                state.set(manager.get_state(&uuid));
            }
        }
    });
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_availability_cannot_replace_update_progress_or_failure() {
        let manager = FirmwareUpdateManager::new();
        let uuid = [1; 6];
        let claim = manager.try_claim(uuid).expect("the device is idle");
        manager.set_before_update_state(uuid, FirmwareUpdateState::Idle);
        assert_eq!(manager.get_state(&uuid), FirmwareUpdateState::Downloading);
        manager.set_state(uuid, FirmwareUpdateState::Failed("Disconnected".into()));
        manager.set_before_update_state(uuid, FirmwareUpdateState::Idle);
        assert_eq!(
            manager.get_state(&uuid),
            FirmwareUpdateState::Failed("Disconnected".into())
        );
        assert!(manager.try_claim(uuid).is_none());
        drop(claim);
        assert!(manager.try_claim(uuid).is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelling_an_unpolled_update_releases_its_claim() {
        let manager = FirmwareUpdateManager::new();
        let uuid = [2; 6];
        let claim = manager.try_claim(uuid).expect("the device is idle");
        let task = tokio::spawn(async move {
            let _claim = claim;
            std::future::pending::<()>().await;
        });
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!manager.is_updating(&uuid));
        assert!(manager.try_claim(uuid).is_some());
    }
}
