use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui::{App, AppContext as _, Entity, Global};
use odyssey_hub_common::device::Device;
use parking_lot::Mutex;

use super::update_state::FirmwareUpdateState;
use crate::hub::HubHandle;

/// Firmware update state per device, shared between the tokio update tasks
/// and the views that show it. `generation` counts every change so the UI can
/// redraw only when something moved.
#[derive(Clone)]
pub(crate) struct FirmwareUpdateManager {
    states: Arc<Mutex<HashMap<[u8; 6], FirmwareUpdateState>>>,
    updating: Arc<Mutex<HashSet<[u8; 6]>>>,
    /// The hub's device list as last seen, which the tokio update task cannot
    /// read from the GPUI-side hub entity.
    devices: Arc<Mutex<Vec<Device>>>,
    generation: Arc<AtomicU64>,
}

/// Notifies observers whenever the manager's state changed. Views observe it
/// instead of polling the manager themselves.
pub(crate) struct UpdateTicker;

struct ManagerHandle {
    manager: FirmwareUpdateManager,
    ticker: Entity<UpdateTicker>,
}

impl Global for ManagerHandle {}

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

impl Default for FirmwareUpdateManager {
    fn default() -> Self {
        Self::new()
    }
}

/// How often the ticker checks whether the manager changed; the update task
/// writes it from another thread.
const STATE_POLL_INTERVAL: Duration = Duration::from_millis(100);

impl FirmwareUpdateManager {
    pub(crate) fn new() -> Self {
        Self {
            states: Arc::new(Mutex::new(HashMap::new())),
            updating: Arc::new(Mutex::new(HashSet::new())),
            devices: Arc::new(Mutex::new(Vec::new())),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The app-wide manager and its ticker. The first call also starts
    /// mirroring the hub's device list, so an update keeps verifying its device
    /// while no page is open.
    pub(crate) fn global(cx: &mut App) -> (Self, Entity<UpdateTicker>) {
        if let Some(handle) = cx.try_global::<ManagerHandle>() {
            return (handle.manager.clone(), handle.ticker.clone());
        }
        let manager = Self::new();
        let ticker = cx.new(|_| UpdateTicker);

        let hub = cx.global::<HubHandle>().0.clone();
        manager.set_devices(hub.read(cx).devices.clone());
        let mirror = manager.clone();
        cx.observe(&hub, move |hub, cx| {
            mirror.set_devices(hub.read(cx).devices.clone());
        })
        .detach();

        let watched = manager.clone();
        let tick = ticker.clone();
        cx.spawn(async move |cx| {
            let mut seen = watched.generation.load(Ordering::Acquire);
            loop {
                cx.background_executor().timer(STATE_POLL_INTERVAL).await;
                let now = watched.generation.load(Ordering::Acquire);
                if now != seen {
                    seen = now;
                    tick.update(cx, |_, cx| cx.notify());
                }
            }
        })
        .detach();

        cx.set_global(ManagerHandle {
            manager: manager.clone(),
            ticker: ticker.clone(),
        });
        (manager, ticker)
    }

    pub(super) fn set_devices(&self, devices: Vec<Device>) {
        *self.devices.lock() = devices;
        self.bump();
    }

    pub(super) fn devices(&self) -> Vec<Device> {
        self.devices.lock().clone()
    }

    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::Release);
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
        self.bump();
    }

    /// Availability is computed during render and may already be stale when it
    /// is ready to publish. Never let it overwrite a task's progress or result.
    pub(super) fn set_before_update_state(&self, uuid: [u8; 6], state: FirmwareUpdateState) {
        let mut states = self.states.lock();
        let current = states.entry(uuid).or_insert(FirmwareUpdateState::Idle);
        // Equal states would bump every render and loop the ticker forever.
        if current.is_before_update() && *current != state {
            *current = state;
            drop(states);
            self.bump();
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

    pub(crate) fn is_updating(&self, uuid: &[u8; 6]) -> bool {
        self.updating.lock().contains(uuid)
    }

    /// The devices being updated, in a stable order.
    pub(crate) fn get_all_updating(&self) -> Vec<[u8; 6]> {
        let mut uuids: Vec<_> = self.updating.lock().iter().copied().collect();
        uuids.sort_unstable();
        uuids
    }

    fn clear_state(&self, uuid: &[u8; 6]) {
        self.states.lock().remove(uuid);
        self.updating.lock().remove(uuid);
        self.bump();
    }
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
