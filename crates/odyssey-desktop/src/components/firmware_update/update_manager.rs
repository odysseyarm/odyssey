use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui::{App, AppContext as _, Entity, Global};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::firmware_version::FirmwareVersion;
use parking_lot::Mutex;

use super::update_state::FirmwareUpdateState;
use crate::hub::HubHandle;

struct DeviceUpdate {
    state: FirmwareUpdateState,
    updating: bool,
}

impl Default for DeviceUpdate {
    fn default() -> Self {
        Self {
            state: FirmwareUpdateState::Idle,
            updating: false,
        }
    }
}

/// Firmware update state per device, shared between the tokio update tasks
/// and the views that show it. `generation` counts every change so the UI can
/// redraw only when something moved.
#[derive(Clone)]
pub(crate) struct FirmwareUpdateManager {
    states: Arc<Mutex<HashMap<[u8; 6], DeviceUpdate>>>,
    /// The hub's device list as last seen, which the tokio update task cannot
    /// read from the GPUI-side hub entity.
    devices: Arc<Mutex<Vec<Device>>>,
    radio_ota_proofs: Arc<
        Mutex<HashMap<[u8; 6], odyssey_hub_common::config::RadioOtaBootloaderProof>>,
    >,
    generation: Arc<AtomicU64>,
    pub(super) dongles: Arc<Mutex<super::dongle_update::DongleUpdates>>,
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
        self.manager.release_claim(self.uuid);
    }
}

impl Default for FirmwareUpdateManager {
    fn default() -> Self {
        Self::new()
    }
}

/// How often the ticker checks whether the manager changed. The update task
/// writes it from another thread.
const STATE_POLL_INTERVAL: Duration = Duration::from_millis(100);

impl FirmwareUpdateManager {
    pub(crate) fn new() -> Self {
        Self {
            states: Arc::new(Mutex::new(HashMap::new())),
            devices: Arc::new(Mutex::new(Vec::new())),
            radio_ota_proofs: Arc::new(Mutex::new(HashMap::new())),
            generation: Arc::new(AtomicU64::new(0)),
            dongles: Arc::new(Mutex::new(super::dongle_update::DongleUpdates::default())),
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
        let proof_manager = manager.clone();
        crate::runtime::spawn(async move {
            let state = odyssey_hub_common::config::ui_state_async()
                .await
                .into_value();
            let mut proofs = proof_manager.radio_ota_proofs.lock();
            for (address, proof) in state.radio_ota_bootloader_proofs {
                if let Ok(address) = hex::decode(address)
                    && let Ok(address) = <[u8; 6]>::try_from(address)
                {
                    proofs.insert(address, proof);
                }
            }
            drop(proofs);
            proof_manager.bump();
        });

        let hub = cx.global::<HubHandle>().0.clone();
        manager.set_devices(hub.read(cx).devices.clone());
        manager.set_dongle_versions(hub.read(cx).dongles.clone());
        super::dongle_update::watch_dongles(manager.clone());
        let mirror = manager.clone();
        cx.observe(&hub, move |hub, cx| {
            mirror.set_devices(hub.read(cx).devices.clone());
            mirror.set_dongle_versions(hub.read(cx).dongles.clone());
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

    pub(super) fn radio_ota_bootloader_version(
        &self,
        uuid: [u8; 6],
        app_version: Option<FirmwareVersion>,
        numeric_core: [u32; 3],
    ) -> Option<semver::Version> {
        let mut proofs = self.radio_ota_proofs.lock();
        let bootloader = proofs
            .get(&uuid)
            .and_then(|proof| Self::radio_ota_proof_version(proof, app_version, numeric_core));
        if bootloader.is_none() && proofs.remove(&uuid).is_some() {
            let address = hex::encode(uuid);
            crate::runtime::spawn(async move {
                let _ = odyssey_hub_common::config::ui_state_update_async(move |state| {
                    state.radio_ota_bootloader_proofs.remove(&address);
                })
                .await;
            });
        }
        bootloader
    }

fn radio_ota_proof_version(
    proof: &odyssey_hub_common::config::RadioOtaBootloaderProof,
    app_version: Option<FirmwareVersion>,
    numeric_core: [u32; 3],
) -> Option<semver::Version> {
    let proven_app = proof.app_version.parse::<FirmwareVersion>().ok()?;
    let identity_matches = match app_version {
        Some(current) => current.to_string() == proof.app_version,
        None => numeric_core != [0, 0, 0] && numeric_core == proven_app.core,
    };
    if !identity_matches {
        return None;
    }
    semver::Version::parse(&proof.bootloader_version)
        .ok()
        .filter(|version| *version >= semver::Version::new(0, 2, 0))
}

    pub(super) fn record_radio_ota_proof(
        &self,
        uuid: [u8; 6],
        app_version: FirmwareVersion,
        bootloader_version: semver::Version,
    ) {
        if bootloader_version < semver::Version::new(0, 2, 0) {
            return;
        }
        let proof = odyssey_hub_common::config::RadioOtaBootloaderProof {
            app_version: app_version.to_string(),
            bootloader_version: bootloader_version.to_string(),
        };
        self.radio_ota_proofs.lock().insert(uuid, proof.clone());
        let address = hex::encode(uuid);
        crate::runtime::spawn(async move {
            if let Err(error) = odyssey_hub_common::config::ui_state_update_async(move |state| {
                state.radio_ota_bootloader_proofs.insert(address, proof);
            })
            .await
            {
                tracing::error!("Could not persist radio OTA compatibility proof: {error}");
            }
        });
        self.bump();
    }

    pub(super) fn bump(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub(super) fn get_state(&self, uuid: [u8; 6]) -> FirmwareUpdateState {
        self.states
            .lock()
            .get(&uuid)
            .map(|device| device.state.clone())
            .unwrap_or(FirmwareUpdateState::Idle)
    }

    pub(super) fn set_state(&self, uuid: [u8; 6], state: FirmwareUpdateState) {
        self.states.lock().entry(uuid).or_default().state = state;
        self.bump();
    }

    /// Availability is computed during render and may already be stale when it
    /// is ready to publish. Never let it overwrite a task's progress or result.
    pub(super) fn set_before_update_state(&self, uuid: [u8; 6], state: FirmwareUpdateState) {
        let mut states = self.states.lock();
        let current = &mut states.entry(uuid).or_default().state;
        // Equal states would bump every render and loop the ticker forever.
        if current.is_before_update() && *current != state {
            *current = state;
            drop(states);
            self.bump();
        }
    }

    /// Claim a device for the lifetime of one update. State and claim change
    /// under one lock, so two clicks cannot start competing transfers.
    pub(super) fn try_claim(&self, uuid: [u8; 6]) -> Option<UpdateClaim> {
        let mut states = self.states.lock();
        let device = states.entry(uuid).or_default();
        if device.updating {
            return None;
        }
        device.updating = true;
        device.state = FirmwareUpdateState::Downloading;
        drop(states);
        self.bump();
        Some(UpdateClaim {
            manager: self.clone(),
            uuid,
        })
    }

    pub(crate) fn is_updating(&self, uuid: [u8; 6]) -> bool {
        self.states
            .lock()
            .get(&uuid)
            .is_some_and(|device| device.updating)
    }

    pub(crate) fn get_visible_updates(&self) -> Vec<[u8; 6]> {
        let states = self.states.lock();
        let mut uuids: Vec<_> = states
            .iter()
            .filter_map(|(uuid, device)| {
                (device.updating
                    || matches!(
                        &device.state,
                        FirmwareUpdateState::Failed(_) | FirmwareUpdateState::Complete
                    ))
                .then_some(*uuid)
            })
            .collect();
        uuids.sort_unstable();
        uuids
    }

    pub(super) fn dismiss_result(&self, uuid: [u8; 6]) {
        let mut states = self.states.lock();
        let Some(device) = states.get_mut(&uuid) else {
            return;
        };
        if device.updating
            || !matches!(
                &device.state,
                FirmwareUpdateState::Failed(_) | FirmwareUpdateState::Complete
            )
        {
            return;
        }
        device.state = FirmwareUpdateState::Idle;
        drop(states);
        self.bump();
    }

    pub(super) fn retry_failed(&self, uuid: [u8; 6]) {
        let mut states = self.states.lock();
        let Some(device) = states.get_mut(&uuid) else {
            return;
        };
        if device.updating || !matches!(&device.state, FirmwareUpdateState::Failed(_)) {
            return;
        }
        device.state = FirmwareUpdateState::Idle;
        drop(states);
        self.bump();
    }

    fn release_claim(&self, uuid: [u8; 6]) {
        let mut states = self.states.lock();
        if let Some(device) = states.get_mut(&uuid) {
            if !matches!(
                &device.state,
                FirmwareUpdateState::Failed(_) | FirmwareUpdateState::Complete
            ) {
                device.state = FirmwareUpdateState::Idle;
            }
            device.updating = false;
        }
        drop(states);
        self.bump();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_usb_mux_identity_reuses_only_matching_persisted_proof() {
        let proof = odyssey_hub_common::config::RadioOtaBootloaderProof {
            app_version: "1.2.3-nightly.20261111.t010000.g000000000001".into(),
            bootloader_version: "0.2.0".into(),
        };
        assert!(
            FirmwareUpdateManager::radio_ota_proof_version(&proof, None, [1, 2, 3]).is_some()
        );
        assert!(
            FirmwareUpdateManager::radio_ota_proof_version(&proof, None, [1, 2, 4]).is_none()
        );
        assert!(
            FirmwareUpdateManager::radio_ota_proof_version(
                &proof,
                Some("1.2.3-nightly.20261111.t020000.g000000000002".parse().unwrap()),
                [1, 2, 3],
            )
            .is_none()
        );
        assert!(
            FirmwareUpdateManager::radio_ota_proof_version(
                &proof,
                Some(proof.app_version.parse().unwrap()),
                [1, 2, 3],
            )
            .is_some()
        );
    }
    #[test]
    fn retry_preserves_active_transfer_and_resets_only_failure() {
        let manager = FirmwareUpdateManager::new();
        let uuid = [3; 6];
        let claim = manager.try_claim(uuid).unwrap();
        assert_eq!(manager.get_visible_updates(), vec![uuid]);
        manager.set_state(uuid, FirmwareUpdateState::Failed("USB lost".into()));
        manager.retry_failed(uuid);
        assert!(matches!(
            manager.get_state(uuid),
            FirmwareUpdateState::Failed(_)
        ));
        manager.dismiss_result(uuid);
        assert!(matches!(
            manager.get_state(uuid),
            FirmwareUpdateState::Failed(_)
        ));
        drop(claim);
        assert_eq!(manager.get_visible_updates(), vec![uuid]);
        manager.retry_failed(uuid);
        assert_eq!(manager.get_state(uuid), FirmwareUpdateState::Idle);
        assert_eq!(manager.get_visible_updates(), Vec::<[u8; 6]>::new());
        manager.set_state(uuid, FirmwareUpdateState::Complete);
        manager.retry_failed(uuid);
        assert_eq!(manager.get_state(uuid), FirmwareUpdateState::Complete);
        assert_eq!(manager.get_visible_updates(), vec![uuid]);
        manager.dismiss_result(uuid);
        assert_eq!(manager.get_visible_updates(), Vec::<[u8; 6]>::new());
    }

    #[test]
    fn stale_availability_cannot_replace_update_progress_or_failure() {
        let manager = FirmwareUpdateManager::new();
        let uuid = [1; 6];
        let claim = manager.try_claim(uuid).expect("the device is idle");
        manager.set_before_update_state(uuid, FirmwareUpdateState::Idle);
        assert_eq!(manager.get_state(uuid), FirmwareUpdateState::Downloading);
        manager.set_state(uuid, FirmwareUpdateState::Failed("Disconnected".into()));
        manager.set_before_update_state(uuid, FirmwareUpdateState::Idle);
        assert_eq!(
            manager.get_state(uuid),
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
        assert!(!manager.is_updating(uuid));
        assert!(manager.try_claim(uuid).is_some());
    }
}
