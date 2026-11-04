use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use super::update_state::FirmwareUpdateState;

/// Global state manager for firmware updates
/// This is shared between tokio tasks and Dioxus components
#[derive(Clone)]
pub(crate) struct FirmwareUpdateManager {
    states: Arc<Mutex<HashMap<[u8; 6], FirmwareUpdateState>>>,
    updating: Arc<Mutex<std::collections::HashSet<[u8; 6]>>>,
}

impl PartialEq for FirmwareUpdateManager {
    fn eq(&self, other: &Self) -> bool {
        // Compare by Arc pointer - same manager instance
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
            updating: Arc::new(Mutex::new(std::collections::HashSet::new())),
        }
    }

    pub(crate) fn get_state(&self, uuid: &[u8; 6]) -> FirmwareUpdateState {
        self.states
            .lock()
            .get(uuid)
            .cloned()
            .unwrap_or(FirmwareUpdateState::Idle)
    }

    pub(crate) fn set_state(&self, uuid: [u8; 6], state: FirmwareUpdateState) {
        self.states.lock().insert(uuid, state);
    }

    /// Claim a device for an update. Returns false when one is already running,
    /// so the check and the insert cannot race two clicks against each other.
    pub(crate) fn try_claim(&self, uuid: [u8; 6]) -> bool {
        self.updating.lock().insert(uuid)
    }

    pub(crate) fn is_updating(&self, uuid: &[u8; 6]) -> bool {
        self.updating.lock().contains(uuid)
    }

    pub(crate) fn get_all_updating(&self) -> Vec<[u8; 6]> {
        self.updating.lock().iter().copied().collect()
    }

    pub(crate) fn clear_state(&self, uuid: &[u8; 6]) {
        self.states.lock().remove(uuid);
        self.updating.lock().remove(uuid);
    }
}
