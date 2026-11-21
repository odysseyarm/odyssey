//! Firmware update state shared by the update tasks and the views, plus the
//! bootloader floor that gates Bluetooth updates of ATS Lite units.
//!
//! `update_task.rs` runs updates on tokio and writes progress here; the views
//! (`device_firmware_update.rs`) read it and redraw when the ticker fires.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui::{App, AppContext as _, Entity, Global};
use odyssey_hub_common::device::Device;
use parking_lot::Mutex;

use super::update_state::FirmwareUpdateState;
use crate::hub::HubHandle;

/// Lowest ATS Lite stage2 bootloader that Bluetooth (radio) OTA may rely on:
/// `0.2.0`, stage2 v3.
///
/// The released stage2 bootloaders, v1 and v2, do not bound their net-core
/// install retries. A net-core update that goes wrong over Bluetooth can leave
/// such a unit retrying in the bootloader with only USB left to recover it. v3
/// bounds the retries, so every Bluetooth update of an ATS Lite is allowed only
/// for a unit that reports v3 or newer.
///
/// The unit reports its bootloader through the device's `BootloaderVersion`
/// property (`Device::bootloader_version`), read from the record the bootloader
/// leaves in flash. A bootloader that predates the record reports nothing, so
/// such a unit is updated over USB, which can rewrite the bootloader, until it
/// runs one that carries the record. [`radio_ota_bootloader_version`] turns the
/// reading into the version the gates use:
///
/// - `ats_ble_update_needs_usb` in `update_check.rs` shows "Plug in USB"
///   instead of an update button when there is none.
/// - `run_radio_ota` in `update_task.rs` refuses to start without one.
/// - `firmware::ota::send` in odyssey-hub-server receives it as
///   `known_bootloader_floor` and refuses a release whose `requires_bootloader`
///   is above it.
///
/// The marker has no stage2 and is not subject to this gate.
pub(super) const RADIO_OTA_MIN_BOOTLOADER: semver::Version = semver::Version::new(0, 2, 0);

/// The bootloader version `device` reports, or `None` when it reports none or
/// one below [`RADIO_OTA_MIN_BOOTLOADER`].
pub(super) fn radio_ota_bootloader_version(device: &Device) -> Option<semver::Version> {
    let [major, minor, patch] = device.bootloader_version;
    let version = semver::Version::new(u64::from(major), u64::from(minor), u64::from(patch));
    (version >= RADIO_OTA_MIN_BOOTLOADER).then_some(version)
}

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

    pub(super) fn bump(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub(super) fn get_state(&self, uuid: [u8; 6]) -> FirmwareUpdateState {
        self.states
            .lock()
            .get(&uuid)
            .map_or(FirmwareUpdateState::Idle, |device| device.state.clone())
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
    use odyssey_hub_common::device::{DeviceCapabilities, EventsTransport, Transport};
    use odyssey_hub_common::firmware_version::FirmwareVersion;

    use super::*;

    fn device_reporting(bootloader_version: [u32; 3]) -> Device {
        Device {
            uuid: [4; 6],
            transport: Transport::UsbMux,
            capabilities: DeviceCapabilities::empty(),
            firmware_identity: FirmwareVersion::default(),
            firmware_version: [1, 2, 3],
            events_transport: EventsTransport::Bluetooth,
            events_connected: false,
            product_id: 0x5211,
            name: Device::name_bytes(""),
            netcore_release: 0,
            bootloader_version,
        }
    }

    #[test]
    fn bluetooth_gate_follows_the_reported_bootloader() {
        assert_eq!(
            radio_ota_bootloader_version(&device_reporting([0, 0, 0])),
            None
        );
        assert_eq!(
            radio_ota_bootloader_version(&device_reporting([0, 1, 9])),
            None
        );
        assert_eq!(
            radio_ota_bootloader_version(&device_reporting([0, 2, 0])),
            Some(RADIO_OTA_MIN_BOOTLOADER)
        );
        assert_eq!(
            radio_ota_bootloader_version(&device_reporting([1, 0, 3])),
            Some(semver::Version::new(1, 0, 3))
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
