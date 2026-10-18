//! Firmware update UI components for individual devices.
//!
//! Architecture:
//! - Firmware update state is stored in a global Arc<Mutex<HashMap>> shared between
//!   the tokio runtime and Dioxus components
//! - The actual update runs entirely on tokio::spawn, independent of Dioxus component lifecycle
//! - Dioxus components poll the shared state periodically to update the UI

use dioxus::{logger::tracing, prelude::*};
use slab::Slab;
use odyssey_hub_common::device::{Device, DeviceCapabilities, DeviceCapabilities as DC};
use odyssey_hub_server::firmware::{self, FirmwareManifest};
use std::collections::HashMap;
use parking_lot::Mutex;
use std::sync::Arc;

/// The firmware channel in effect, `None` until the saved choice has loaded.
/// Settings writes it and the manifest fetch follows it, so a change applies
/// without a restart.
pub static FIRMWARE_CHANNEL: GlobalSignal<Option<firmware::FirmwareChannel>> =
    Signal::global(|| None);

/// Fill [`FIRMWARE_CHANNEL`] from the saved choice if nothing has set it yet.
///
/// The check repeats after the load: a user can pick a channel while the saved
/// one is still being read, and that pick must survive.
pub async fn ensure_firmware_channel_loaded() {
    if FIRMWARE_CHANNEL.peek().is_some() {
        return;
    }
    let saved = firmware::FirmwareChannel::load().await;
    let mut channel = FIRMWARE_CHANNEL.write();
    if channel.is_none() {
        *channel = Some(saved);
    }
}

/// Where Odyssey Desktop releases are published.
const DESKTOP_RELEASES_URL: &str = "https://github.com/odysseyarm/odyssey/releases";

/// The Odyssey Desktop version this build is, for matching against the oldest
/// desktop each firmware release declares it needs.
fn desktop_version() -> [u16; 3] {
    [
        env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap_or(0),
        env!("CARGO_PKG_VERSION_MINOR").parse().unwrap_or(0),
        env!("CARGO_PKG_VERSION_PATCH").parse().unwrap_or(0),
    ]
}

/// A firmware release newer than the one on offer that this desktop is too old
/// to install. The user decides whether to update the desktop.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldBack {
    pub version: String,
    pub min_desktop: String,
}

fn held_back_note(held_back: &HeldBack) -> Element {
    rsx! {
        span {
            class: "text-xs text-gray-500 dark:text-gray-400 italic",
            "v{held_back.version} needs Odyssey Desktop {held_back.min_desktop} or newer. "
            a {
                class: "underline text-blue-600 dark:text-blue-400",
                href: DESKTOP_RELEASES_URL,
                "Get the latest"
            }
        }
    }
}

/// State for a single device's firmware update
#[derive(Clone, Debug, PartialEq)]
pub enum FirmwareUpdateState {
    /// No update available or not yet checked
    Idle,
    /// Update is available but device lacks USB control (needs to be plugged in via USB)
    NeedsUsbConnection {
        current: [u16; 3],
        available: String,
    },
    /// Update is available
    Available {
        current: [u16; 3],
        available: String,
        device_type: String,
        vid: u16,
        pid: u16,
        held_back: Option<HeldBack>,
    },
    /// A newer firmware exists but this desktop is too old to install any of it
    NeedsNewerDesktop(HeldBack),
    /// Currently downloading firmware
    Downloading,
    /// Sending DFU detach command
    Detaching,
    /// Waiting for device to enter DFU mode
    WaitingForDfu,
    /// Flashing firmware with progress (bytes_written, total_bytes)
    Flashing { progress: usize, total: usize },
    /// Writing the net core (BLE controller) image with progress (bytes_written, total_bytes)
    FlashingNetCore { progress: usize, total: usize },
    /// Waiting for the net core to report the release that was just written
    VerifyingNetCore,
    /// Waiting for the net core bootloader to copy the staged net core image and
    /// the device to return to DFU mode
    InstallingNetCore,
    /// Waiting for device to reboot after flash
    Rebooting,
    /// Verifying the new firmware version
    Verifying,
    /// Update completed successfully
    Complete,
    /// Update failed
    Failed(String),
}

/// Global state manager for firmware updates
/// This is shared between tokio tasks and Dioxus components
#[derive(Clone)]
pub struct FirmwareUpdateManager {
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
    pub fn new() -> Self {
        Self {
            states: Arc::new(Mutex::new(HashMap::new())),
            updating: Arc::new(Mutex::new(std::collections::HashSet::new())),
        }
    }

    pub fn get_state(&self, uuid: &[u8; 6]) -> FirmwareUpdateState {
        self.states
            .lock()
            .get(uuid)
            .cloned()
            .unwrap_or(FirmwareUpdateState::Idle)
    }

    pub fn set_state(&self, uuid: [u8; 6], state: FirmwareUpdateState) {
        self.states.lock().insert(uuid, state);
    }

    /// Claim a device for an update. Returns false when one is already running,
    /// so the check and the insert cannot race two clicks against each other.
    pub fn try_claim(&self, uuid: [u8; 6]) -> bool {
        self.updating.lock().insert(uuid)
    }

    pub fn is_updating(&self, uuid: &[u8; 6]) -> bool {
        self.updating.lock().contains(uuid)
    }

    pub fn get_all_updating(&self) -> Vec<[u8; 6]> {
        self.updating.lock().iter().copied().collect()
    }

    pub fn clear_state(&self, uuid: &[u8; 6]) {
        self.states.lock().remove(uuid);
        self.updating.lock().remove(uuid);
    }
}

/// Props for the device firmware update button/indicator
#[derive(Props, Clone, PartialEq)]
pub struct DeviceFirmwareUpdateProps {
    /// The device to check/update
    pub device: Device,
    /// Cached firmware manifest (shared across all devices)
    pub manifest: Signal<Option<FirmwareManifest>>,
    /// Firmware update manager
    pub manager: FirmwareUpdateManager,
    /// Live device list, for reading the net core release back after it is written
    pub devices: SyncSignal<Slab<Device>>,
}

/// A component that shows firmware update status and allows updating a single device
#[component]
pub fn DeviceFirmwareUpdate(props: DeviceFirmwareUpdateProps) -> Element {
    let device = props.device.clone();
    let manifest = props.manifest;
    let manager = props.manager.clone();
    let device_uuid = device.uuid;

    // Local state that we poll from the manager
    let mut local_state = use_signal(|| manager.get_state(&device_uuid));

    // Poll the manager for state updates every 100ms
    {
        let manager = manager.clone();
        use_future(move || {
            let manager = manager.clone();
            async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let new_state = manager.get_state(&device_uuid);
                    local_state.set(new_state);
                }
            }
        });
    }

    // Check if update is available - runs on every render when props change
    // We do this synchronously during render rather than in use_effect because
    // use_effect captures values and won't see prop changes
    {
        let current_state = manager.get_state(&device_uuid);

        // Only update state if we're not in the middle of an update
        if matches!(
            current_state,
            FirmwareUpdateState::Idle
                | FirmwareUpdateState::Available { .. }
                | FirmwareUpdateState::NeedsUsbConnection { .. }
                | FirmwareUpdateState::NeedsNewerDesktop(_)
        ) {
            let mut next_state = FirmwareUpdateState::Idle;

            if let Some(manifest) = manifest() {
                let device_version = device.firmware_version;
                let pid = device.product_id;
                // True only when the device is stuck in DFU bootloader (no CONTROL).
                // App-mode devices expose a DFU interface too but are NOT in bootloader mode.
                let is_dfu_device = device.capabilities.contains(DC::DFU)
                    && !device.capabilities.contains(DC::CONTROL);

                // Skip if no product ID or no version (version is always readable via control protocol,
                // so [0,0,0] only occurs for devices that are already in DFU bootloader mode).
                if pid == 0 || device_version == [0, 0, 0] && !is_dfu_device {
                    // No product ID, or version unreadable and no DFU interface — skip
                } else
                // Look up device type from its product ID
                if let Some((vid, pid, device_type)) = firmware::device_info_from_pid(pid) {
                    // DFU-bootloader devices (no CONTROL, version [0,0,0]) — show latest firmware.
                    // Connected devices — show latest firmware only if it is actually newer,
                    // allowing cross-major/minor updates (e.g. protocol upgrades).
                    let offer = firmware::select_firmware(&manifest, device_type, desktop_version());
                    let is_newer = |fw: &firmware::FirmwareEntry| {
                        is_dfu_device
                            || fw.version_parts().is_some_and(|v| firmware::version_newer(device_version, v))
                    };
                    let maybe_fw = offer.offered.filter(|fw| is_newer(fw));
                    let held_back = offer.held_back.filter(|fw| is_newer(fw)).map(|fw| HeldBack {
                        version: fw.version.clone(),
                        min_desktop: fw.min_desktop.clone(),
                    });

                    if let Some(available_fw) = maybe_fw {
                        let available_version = available_fw.version.clone();
                        let device_type = device_type.to_string();

                        // DFU devices always show the update button (they need to be flashed to
                        // recover).  Normal devices need CONTROL capability for USB flashing.
                        if is_dfu_device || device.capabilities.contains(DeviceCapabilities::CONTROL) {
                            let new_state = FirmwareUpdateState::Available {
                                current: device_version,
                                available: available_version.clone(),
                                device_type,
                                vid,
                                pid,
                                held_back: held_back.clone(),
                            };
                            if !matches!(current_state, FirmwareUpdateState::Available { .. }) {
                                if is_dfu_device {
                                    tracing::info!(
                                        "DFU device {:02x?} (PID 0x{:04x}) ready to flash -> {}",
                                        device_uuid,
                                        pid,
                                        available_version
                                    );
                                } else {
                                    tracing::info!(
                                        "Update available for {:02x?}: {}.{}.{} -> {}",
                                        device_uuid,
                                        device_version[0],
                                        device_version[1],
                                        device_version[2],
                                        available_version
                                    );
                                }
                            }
                            next_state = new_state;
                        } else {
                            // Device doesn't have USB control - prompt user to plug in USB
                            let new_state = FirmwareUpdateState::NeedsUsbConnection {
                                current: device_version,
                                available: available_version.clone(),
                            };
                            if !matches!(
                                current_state,
                                FirmwareUpdateState::NeedsUsbConnection { .. }
                            ) {
                                tracing::info!(
                                    "Update available for {:02x?}: {}.{}.{} -> {} (needs USB connection)",
                                    device_uuid,
                                    device_version[0],
                                    device_version[1],
                                    device_version[2],
                                    available_version
                                );
                            }
                            next_state = new_state;
                        }
                    } else if let Some(held_back) = held_back {
                        next_state = FirmwareUpdateState::NeedsNewerDesktop(held_back);
                    }
                }
            }

            manager.set_state(device_uuid, next_state);
        }
    }

    let state = local_state();

    match state {
        FirmwareUpdateState::Idle => rsx! {},
        FirmwareUpdateState::NeedsUsbConnection { current, available } => {
            rsx! {
                div {
                    class: "flex items-center gap-2",
                    span {
                        class: "text-xs text-yellow-600 dark:text-yellow-400",
                        "Update available: v{current[0]}.{current[1]}.{current[2]} → v{available}"
                    }
                    span {
                        class: "text-xs text-gray-500 dark:text-gray-400 italic",
                        "Plug in USB to update"
                    }
                }
            }
        }
        FirmwareUpdateState::NeedsNewerDesktop(held_back) => held_back_note(&held_back),
        FirmwareUpdateState::Available {
            current,
            available,
            device_type,
            vid,
            pid,
            held_back,
        } => {
            let manager = props.manager.clone();
            let devices = props.devices;
            let manifest = manifest.clone();
            let device = device.clone();
            let is_dfu_bootloader = device.capabilities.contains(DC::DFU) && !device.capabilities.contains(DC::CONTROL);

            rsx! {
                div {
                    class: "flex items-center gap-2",
                    span {
                        class: "text-xs text-yellow-600 dark:text-yellow-400",
                        if is_dfu_bootloader { "Device in DFU mode — flash v{available}" }
                        else { "Update available: v{current[0]}.{current[1]}.{current[2]} → v{available}" }
                    }
                    if let Some(held_back) = &held_back {
                        {held_back_note(held_back)}
                    }
                    button {
                        // The state below is polled, so a second click can land
                        // before this branch stops being rendered.
                        disabled: manager.is_updating(&device.uuid),
                        class: "btn-primary-sm",
                        onclick: move |_| {
                            let manager = manager.clone();
                            let manifest = manifest.clone();
                            let device = device.clone();
                            let device_type = device_type.clone();

                            // Start the update - runs entirely on tokio
                            start_firmware_update(
                                manager,
                                manifest(),
                                device,
                                device_type,
                                vid,
                                pid,
                                devices,
                            );
                        },
                        "Update"
                    }
                }
            }
        }
        FirmwareUpdateState::Downloading => rsx! {
            span { class: "text-xs text-blue-500", "Downloading..." }
        },
        FirmwareUpdateState::Detaching => rsx! {
            span { class: "text-xs text-blue-500", "Preparing device..." }
        },
        FirmwareUpdateState::WaitingForDfu => rsx! {
            span { class: "text-xs text-blue-500", "Waiting for DFU mode..." }
        },
        FirmwareUpdateState::Flashing { progress, total } => {
            let percent = if total > 0 {
                (progress * 100) / total
            } else {
                0
            };
            rsx! {
                div {
                    class: "flex flex-col gap-1 min-w-32",
                    span { class: "text-xs text-blue-500", "Flashing... {percent}%" }
                    div {
                        class: "w-full bg-gray-200 dark:bg-gray-700 rounded-full h-2",
                        div {
                            class: "bg-blue-600 h-2 rounded-full transition-all duration-100",
                            style: "width: {percent}%",
                        }
                    }
                    span { class: "text-xs text-gray-400", "{progress / 1024} / {total / 1024} KB" }
                }
            }
        }
        FirmwareUpdateState::FlashingNetCore { progress, total } => {
            let percent = if total > 0 {
                (progress * 100) / total
            } else {
                0
            };
            rsx! {
                div {
                    class: "flex flex-col gap-1 min-w-32",
                    span { class: "text-xs text-blue-500", "Updating radio firmware... {percent}%" }
                    div {
                        class: "w-full bg-gray-200 dark:bg-gray-700 rounded-full h-2",
                        div {
                            class: "bg-blue-600 h-2 rounded-full transition-all duration-100",
                            style: "width: {percent}%",
                        }
                    }
                    span { class: "text-xs text-gray-400", "{progress / 1024} / {total / 1024} KB" }
                }
            }
        }
        FirmwareUpdateState::VerifyingNetCore => rsx! {
            span { class: "text-xs text-blue-500", "Checking radio firmware..." }
        },
        FirmwareUpdateState::InstallingNetCore => rsx! {
            span {
                class: "text-xs text-blue-500",
                "Installing radio firmware (this can take a couple of minutes)..."
            }
        },
        FirmwareUpdateState::Rebooting => rsx! {
            span { class: "text-xs text-blue-500", "Rebooting device..." }
        },
        FirmwareUpdateState::Verifying => rsx! {
            span { class: "text-xs text-blue-500", "Verifying update..." }
        },
        FirmwareUpdateState::Complete => rsx! {
            span { class: "text-xs text-green-500", "Update complete!" }
        },
        FirmwareUpdateState::Failed(ref error) => rsx! {
            div {
                class: "flex flex-col gap-1",
                span { class: "text-xs text-red-500 font-semibold", "Update failed" }
                span { class: "text-xs text-red-400", "{error}" }
            }
        },
    }
}

/// Start a firmware update - runs entirely on tokio runtime
fn start_firmware_update(
    manager: FirmwareUpdateManager,
    manifest: Option<FirmwareManifest>,
    device: Device,
    device_type: String,
    vid: u16,
    pid: u16,
    devices: SyncSignal<Slab<Device>>,
) {
    let device_uuid = device.uuid;

    // Two tasks driving DFU on one device would interleave their control
    // transfers and corrupt the download.
    if !manager.try_claim(device_uuid) {
        tracing::warn!("Firmware update already running for {:02x?}", device_uuid);
        return;
    }

    // Spawn the entire update on tokio - completely independent of Dioxus
    tokio::spawn(async move {
        let result =
            run_firmware_update(&manager, manifest, device, device_type, vid, pid, devices).await;

        if let Err(e) = result {
            tracing::error!("Firmware update failed: {}", e);
            manager.set_state(device_uuid, FirmwareUpdateState::Failed(e));
        }

        // Clean up after a delay so user can see final state
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        manager.clear_state(&device_uuid);
    });
}

/// How long the net core gets to report the release that was just written.
///
/// The application only learns the release once its BLE controller has
/// answered, and the server's own probe retries for about half a minute after
/// the device reconnects, so this has to outlast both.
const NETCORE_REPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// How long the net core bootloader and the stage2 bootloader get to finish a
/// net core update and bring the device back into DFU mode. Stage2 itself gives
/// the copy up to two minutes before it resets to retry, so this has to outlast
/// that.
const NETCORE_INSTALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(150);

/// How long the device may keep enumerating as a bootloader after the net core
/// download completes before it counts as never having reset to start the copy.
const NETCORE_RESET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

fn netcore_not_reported(published: u16) -> String {
    format!(
        "the net core did not report release {published}; the device may have no radio, \
         run the update again"
    )
}

/// The net core release the hub last read from a device that is in application
/// mode, `None` while the device is absent or has no control connection.
fn reported_netcore_release(devices: &SyncSignal<Slab<Device>>, uuid: &[u8; 6]) -> Option<u16> {
    devices
        .peek()
        .iter()
        .find(|(_, d)| &d.uuid == uuid && d.capabilities.contains(DC::CONTROL))
        .map(|(_, d)| d.netcore_release)
}

/// Poll the hub's device list until the net core reports exactly `published`.
///
/// The hub rereads the release each time the device reconnects, so a reading
/// from before the reset can only be a mismatch, never a false match: the old
/// release differs from the one just written, and a device with no answer reads 0.
async fn wait_for_netcore_release(
    devices: &SyncSignal<Slab<Device>>,
    uuid: &[u8; 6],
    published: u16,
) -> Result<(), String> {
    let start = std::time::Instant::now();
    let mut last = None;
    while start.elapsed() < NETCORE_REPORT_TIMEOUT {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Some(now) = reported_netcore_release(devices, uuid) {
            last = Some(now);
            if firmware::netcore_took_update(now, published) {
                tracing::info!("Net core now reports release {}", now);
                return Ok(());
            }
        }
    }
    tracing::warn!(
        "Net core did not report release {} (last reading: {:?})",
        published,
        last
    );
    Err(netcore_not_reported(published))
}

/// Poll until the device enumerates in application mode, `false` on timeout.
async fn wait_for_app_mode(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
    timeout: std::time::Duration,
) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if firmware::is_device_in_app_mode(uuid, vid, pid) {
            return true;
        }
    }
    false
}

/// Put the device into DFU mode, detaching it first when it is running its
/// application.
async fn enter_dfu(
    manager: &FirmwareUpdateManager,
    device_uuid: [u8; 6],
    vid: u16,
    pid: u16,
) -> Result<(), String> {
    // The application also exposes a DFU interface (for the runtime descriptor
    // that takes the detach), so only the DFU-mode protocol means the bootloader
    // is already running and waiting for a download. It enumerates under its own
    // product id, which is why the application's id alone cannot find it.
    let already_in_dfu = firmware::is_device_in_dfu_mode(&device_uuid, vid, pid);

    if already_in_dfu {
        tracing::info!("Device {:02x?} is already in DFU mode, skipping detach", device_uuid);
        return Ok(());
    }

    // Send DFU detach
    tracing::info!("Sending DFU detach to {:04x}:{:04x} (uuid {:02x?})", vid, pid, device_uuid);
    manager.set_state(device_uuid, FirmwareUpdateState::Detaching);
    firmware::send_dfu_detach(&device_uuid, vid, pid)
        .await
        .map_err(|e| format!("DFU detach failed: {}", e))?;

    // Wait for device to enter DFU mode
    tracing::info!("Waiting for device to enter DFU mode");
    manager.set_state(device_uuid, FirmwareUpdateState::WaitingForDfu);
    firmware::wait_for_dfu_device(&device_uuid, vid, pid, std::time::Duration::from_secs(10))
        .await
        .map_err(|e| format!("Device did not enter DFU mode: {}", e))
}

/// Write one image to a DFU alt setting, publishing progress as it goes.
async fn flash_with_progress(
    manager: &FirmwareUpdateManager,
    device_uuid: [u8; 6],
    vid: u16,
    pid: u16,
    slot: &str,
    alt: u8,
    data: &[u8],
    what: &str,
    progress_state: fn(usize, usize) -> FirmwareUpdateState,
) -> Result<(), String> {
    let total_size = data.len();
    manager.set_state(device_uuid, progress_state(0, total_size));

    // Use atomic counter for progress
    let progress_bytes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let progress_bytes_clone = progress_bytes.clone();
    let manager_clone = manager.clone();

    // Spawn progress updater
    let progress_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let current = progress_bytes_clone.load(std::sync::atomic::Ordering::Relaxed);
            manager_clone.set_state(device_uuid, progress_state(current, total_size));
            if current >= total_size {
                break;
            }
        }
    });

    // Flash
    let progress_callback = {
        let progress_bytes = progress_bytes.clone();
        move |bytes_written: usize, _total: usize| {
            progress_bytes.store(bytes_written, std::sync::atomic::Ordering::Relaxed);
        }
    };

    let flash_result = firmware::flash_firmware_with_progress(
        &device_uuid,
        vid,
        pid,
        slot,
        alt,
        data,
        progress_callback,
    )
    .await;

    // Stop progress updater
    progress_task.abort();

    flash_result.map_err(|e| format!("{what} failed: {e}"))?;

    // Verify flash was complete
    let final_progress = progress_bytes.load(std::sync::atomic::Ordering::Relaxed);
    if final_progress < total_size {
        return Err(format!(
            "{what} incomplete: {final_progress}/{total_size} bytes written"
        ));
    }
    Ok(())
}

/// Poll until the device stops enumerating as a bootloader, `false` on timeout.
async fn wait_for_bootloader_reset(
    uuid: &[u8; 6],
    vid: u16,
    pid: u16,
    timeout: std::time::Duration,
) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !firmware::is_device_in_dfu_mode(uuid, vid, pid) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    false
}

/// Write the net core image and wait for the device to be back in DFU mode.
///
/// Completing the download only means the bootloader has the image staged. The
/// device resets, and the stage2 bootloader starts the net core bootloader,
/// which copies the image into net flash; stage2 treats every outcome of that
/// copy alike. The application's DFU request is still pending afterwards, so the
/// device returns to DFU mode and stays there until an application is written:
/// the caller continues in that same DFU session without detaching again. Nothing
/// reports the net core while the device is in the bootloader, so whether the
/// copy took is only readable once the new application runs.
async fn flash_netcore(
    manager: &FirmwareUpdateManager,
    device_uuid: [u8; 6],
    vid: u16,
    pid: u16,
    image: &[u8],
    release: u16,
) -> Result<(), String> {
    enter_dfu(manager, device_uuid, vid, pid).await?;

    let alt = firmware::find_netcore_alt_for_device(&device_uuid, vid, pid)
        .map_err(|e| format!("The net core cannot be updated over DFU: {e}"))?;
    tracing::info!("Updating net core to release {} on alt {}", release, alt);
    flash_with_progress(
        manager,
        device_uuid,
        vid,
        pid,
        "NET",
        alt,
        image,
        "Net core flash",
        |progress, total| FirmwareUpdateState::FlashingNetCore { progress, total },
    )
    .await?;

    manager.set_state(device_uuid, FirmwareUpdateState::InstallingNetCore);
    tracing::info!("Waiting for the device to copy the net core image and return to DFU mode...");
    // The bootloader may still be enumerating for a moment after the download,
    // and that stale enumeration would satisfy the wait below at once, sending
    // the application image into a device that is about to reset.
    if !wait_for_bootloader_reset(&device_uuid, vid, pid, NETCORE_RESET_TIMEOUT).await {
        return Err(format!(
            "Device did not reset within {}s after the net core download. Leave it \
             connected and run the update again.",
            NETCORE_RESET_TIMEOUT.as_secs()
        ));
    }
    firmware::wait_for_dfu_device(&device_uuid, vid, pid, NETCORE_INSTALL_TIMEOUT)
        .await
        .map_err(|e| {
            format!(
                "Device did not return to DFU mode within {}s after the net core update ({e}). \
                 Leave it connected and run the update again.",
                NETCORE_INSTALL_TIMEOUT.as_secs()
            )
        })?;
    tracing::info!("Device is back in DFU mode");
    Ok(())
}

/// Run the actual firmware update sequence
async fn run_firmware_update(
    manager: &FirmwareUpdateManager,
    manifest: Option<FirmwareManifest>,
    device: Device,
    device_type: String,
    vid: u16,
    pid: u16,
    devices: SyncSignal<Slab<Device>>,
) -> Result<(), String> {
    let device_uuid = device.uuid;

    let manifest = manifest.ok_or("No manifest available")?;
    let device_version = device.firmware_version;
    // A device sitting in the bootloader has no CONTROL capability and reports
    // no version, so it takes whatever the manifest offers. Everything else
    // must have reported a version for the button to have been shown.
    let in_bootloader =
        device.capabilities.contains(DC::DFU) && !device.capabilities.contains(DC::CONTROL);
    if !in_bootloader && device_version == [0, 0, 0] {
        return Err("No firmware version".to_string());
    }
    let firmware_entry = firmware::select_firmware(&manifest, &device_type, desktop_version())
        .offered
        .ok_or("No firmware for this device type is installable by this version of Odyssey Desktop")?;

    // The net core is released separately from the application, so the entry
    // only says which release it was tested with. Whether it has to be written
    // is decided here, before anything is downloaded or detached, from the
    // release the hub last read. A reading of 0 means unknown (an application
    // too old to report one, a net core that does not answer, a device in the
    // bootloader) and is replaced whenever a release is published.
    //
    // With the current stage2 nothing reports the net core while the device is
    // in the bootloader, and a net core update leaves the device there until an
    // application is written. So the net core is written first and read back
    // only after the new application runs. `requires_netcore` is judged here
    // against what the update will install, and is confirmed by that read-back
    // after the application is written: a silently failed copy is detected then,
    // as an error telling the user to run the update again.
    let published_netcore = firmware_entry
        .files
        .get("net-core")
        .and_then(|entry| entry.release.map(|release| (entry, release)));
    let steps = firmware::plan_update_steps(
        device.netcore_release,
        published_netcore.map(|(_, release)| release),
        firmware_entry.requires_netcore,
    )
    .map_err(|e| e.to_string())?;
    tracing::info!("Update steps for {:02x?}: {:?}", device_uuid, steps);

    // Which bank to write is only readable once the bootloader is running, so
    // fetch both candidates up front. Detaching is the point of no return: the
    // app marks the boot state for DFU before resetting, and only a completed
    // download clears that mark, so a download that fails after the detach
    // leaves the device in the bootloader across every later reboot. The same
    // holds for the net core image, so every download happens before the first
    // detach.
    manager.set_state(device_uuid, FirmwareUpdateState::Downloading);
    let netcore_image = match published_netcore {
        Some((entry, release)) if steps.contains(&firmware::UpdateStep::FlashNetCore) => {
            tracing::info!("Downloading net-core release {}", release);
            let data = firmware::download_firmware(
                &device_type,
                firmware::ReleaseSource::NetCore { release },
                "net-core",
                &entry.sha256,
            )
            .await
            .map_err(|e| format!("Download of net-core failed: {e}"))?;
            Some((data, release))
        }
        _ => None,
    };
    let mut images: std::collections::HashMap<&'static str, Vec<u8>> =
        std::collections::HashMap::new();
    for slot_file in ["app-a", "app-b"] {
        let Some(file_entry) = firmware_entry.files.get(slot_file) else {
            continue;
        };
        tracing::info!("Downloading {} for {}", slot_file, firmware_entry.version);
        let data = firmware::download_firmware(
            &device_type,
            firmware::ReleaseSource::Application { version: &firmware_entry.version },
            slot_file,
            &file_entry.sha256,
        )
        .await
        .map_err(|e| format!("Download of {slot_file} failed: {e}"))?;
        images.insert(slot_file, data);
    }
    if images.is_empty() {
        return Err("Firmware release has no app-a or app-b image".to_string());
    }
    // The stage2 bootloader is versioned separately and republished unchanged
    // across application releases, so it is only worth downloading when the
    // release says which bootloader release it carries.
    let published_bootloader = firmware_entry
        .files
        .get("stage2-bootloader")
        .and_then(|entry| entry.release.map(|release| (entry, release)));
    let bootloader_image = match published_bootloader {
        Some((entry, release)) => {
            tracing::info!("Release carries stage2 bootloader release {}", release);
            let data = firmware::download_firmware(
                &device_type,
                firmware::ReleaseSource::Bootloader { release },
                "stage2-bootloader",
                &entry.sha256,
            )
            .await
            .map_err(|e| format!("Download of stage2-bootloader failed: {e}"))?;
            Some((data, release))
        }
        None => None,
    };

    for step in steps {
        match step {
            firmware::UpdateStep::FlashNetCore => {
                let (image, release) = netcore_image
                    .as_ref()
                    .ok_or("The update plan flashes the net core but no image was downloaded")?;
                flash_netcore(manager, device_uuid, vid, pid, image, *release).await?;
            }
            firmware::UpdateStep::VerifyNetCore => {
                let (_, release) = netcore_image
                    .as_ref()
                    .ok_or("The update plan verifies the net core but none was written")?;
                manager.set_state(device_uuid, FirmwareUpdateState::VerifyingNetCore);
                wait_for_netcore_release(&devices, &device_uuid, *release).await?;
            }
            firmware::UpdateStep::WriteApplication => {
                write_application(
                    manager,
                    device_uuid,
                    vid,
                    pid,
                    &mut images,
                    bootloader_image.as_ref(),
                    firmware_entry.requires_bootloader,
                )
                .await?;
            }
        }
    }

    tracing::info!("Firmware update complete!");
    manager.set_state(device_uuid, FirmwareUpdateState::Complete);
    Ok(())
}

/// Detach into the bootloader if needed, update the bootloader when the release
/// calls for it, write the application to the inactive bank, and wait for the
/// device to come back running it.
async fn write_application(
    manager: &FirmwareUpdateManager,
    device_uuid: [u8; 6],
    vid: u16,
    pid: u16,
    images: &mut std::collections::HashMap<&'static str, Vec<u8>>,
    bootloader_image: Option<&(Vec<u8>, u16)>,
    requires_bootloader: u16,
) -> Result<(), String> {
    enter_dfu(manager, device_uuid, vid, pid).await?;

    // The bootloader owns the flash writes for the application banks, so an
    // application that needs a newer bootloader has to wait for it. This is the
    // only window where the bootloader is the thing enumerating, which is both
    // why its release is readable and why it can be written at all.
    let running = firmware::read_bootloader_release_for_device(&device_uuid, vid, pid)
        .map_err(|e| format!("Could not read the bootloader release: {e}"))?;
    let flash_bootloader = firmware::plan_bootloader(
        running,
        bootloader_image.map(|(_, release)| *release),
        requires_bootloader,
    )
    .map_err(|e| e.to_string())?;
    match bootloader_image {
        Some((bootloader_data, published)) if flash_bootloader => {
            let published = *published;
            let alt = firmware::find_bootloader_alt_for_device(&device_uuid, vid, pid)
                .map_err(|e| format!("Bootloader cannot be updated over DFU: {e}"))?;
            tracing::info!(
                "Updating bootloader {:?} -> release {} on alt {}",
                running,
                published,
                alt
            );
            manager.set_state(
                device_uuid,
                FirmwareUpdateState::Flashing {
                    progress: 0,
                    total: bootloader_data.len(),
                },
            );
            firmware::flash_firmware_with_progress(
                &device_uuid,
                vid,
                pid,
                "BOOTLOADER",
                alt,
                bootloader_data,
                |_, _| {},
            )
            .await
            .map_err(|e| format!("Bootloader flash failed: {e}"))?;

            // Writing the bootloader resets the device, so wait for it to come
            // back before the application image goes out.
            firmware::wait_for_dfu_device(&device_uuid, vid, pid, std::time::Duration::from_secs(15))
                .await
                .map_err(|e| format!("Device did not return after the bootloader update: {e}"))?;

            // A completed write and a returning device do not show the new
            // image is the one running. The application must not go onto a
            // bootloader that never took the update, so read it back.
            let now = firmware::read_bootloader_release_for_device(&device_uuid, vid, pid)
                .map_err(|e| format!("Could not read the bootloader after updating it: {e}"))?;
            if !firmware::bootloader_took_update(now, published) {
                return Err(format!(
                    "Bootloader reports {now:?} after updating to release {published}"
                ));
            }
            tracing::info!("Bootloader now reports {:?}", now);
        }
        _ => tracing::info!("Bootloader {:?} stays as it is", running),
    }

    // Determine target slot
    let (slot_name, slot_file, alt) = firmware::determine_target_slot_for_device(&device_uuid, vid, pid)
        .map_err(|e| format!("Failed to determine target slot: {}", e))?;

    tracing::info!(
        "Target slot: {} (alt {}), firmware file: {}",
        slot_name,
        alt,
        slot_file
    );

    let firmware_data = images
        .remove(slot_file)
        .ok_or_else(|| format!("Firmware file {} not found in manifest", slot_file))?;

    // Flash firmware with progress updates
    tracing::info!("Flashing firmware to {}", slot_name);
    flash_with_progress(
        manager,
        device_uuid,
        vid,
        pid,
        slot_name,
        alt,
        &firmware_data,
        "Flash",
        |progress, total| FirmwareUpdateState::Flashing { progress, total },
    )
    .await?;

    // Wait for reboot
    manager.set_state(device_uuid, FirmwareUpdateState::Rebooting);
    tracing::info!("Waiting for device to reboot...");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify device reconnected
    manager.set_state(device_uuid, FirmwareUpdateState::Verifying);
    tracing::info!("Verifying firmware update...");

    let timeout = std::time::Duration::from_secs(15);
    if wait_for_app_mode(&device_uuid, vid, pid, timeout).await {
        tracing::info!("Device reconnected in application mode");
        Ok(())
    } else {
        // The byte count above only proves the host finished feeding the
        // writer, so a device that never comes back is the one signal left
        // that the new image is not running.
        Err(format!(
            "Device did not reappear within {}s after flashing. It may still be \
             in the bootloader — leave it connected and try again.",
            timeout.as_secs()
        ))
    }
}

/// Props for the updating device row component
#[derive(Props, Clone, PartialEq)]
pub struct UpdatingDeviceRowProps {
    pub uuid: [u8; 6],
    pub manager: FirmwareUpdateManager,
}

/// A component that displays a device being updated (when not in device list)
#[component]
pub fn UpdatingDeviceRow(props: UpdatingDeviceRowProps) -> Element {
    let uuid = props.uuid;
    let manager = props.manager.clone();

    // Poll state from manager
    let mut state = use_signal(|| manager.get_state(&uuid));

    use_future(move || {
        let manager = manager.clone();
        async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                state.set(manager.get_state(&uuid));
            }
        }
    });

    let current_state = state();

    let state_display = match &current_state {
        FirmwareUpdateState::Idle => "Idle".to_string(),
        FirmwareUpdateState::NeedsUsbConnection { .. } => "Plug in USB to update".to_string(),
        FirmwareUpdateState::NeedsNewerDesktop(_) => "Needs a newer Odyssey Desktop".to_string(),
        FirmwareUpdateState::Available { .. } => "Update available".to_string(),
        FirmwareUpdateState::Downloading => "Downloading...".to_string(),
        FirmwareUpdateState::Detaching => "Preparing device...".to_string(),
        FirmwareUpdateState::WaitingForDfu => "Waiting for DFU mode...".to_string(),
        FirmwareUpdateState::Flashing { progress, total } => {
            let percent = if *total > 0 {
                (*progress * 100) / *total
            } else {
                0
            };
            format!("Flashing... {}%", percent)
        }
        FirmwareUpdateState::FlashingNetCore { progress, total } => {
            let percent = if *total > 0 {
                (*progress * 100) / *total
            } else {
                0
            };
            format!("Updating radio firmware... {}%", percent)
        }
        FirmwareUpdateState::VerifyingNetCore => "Checking radio firmware...".to_string(),
        FirmwareUpdateState::InstallingNetCore => {
            "Installing radio firmware (this can take a couple of minutes)...".to_string()
        }
        FirmwareUpdateState::Rebooting => "Rebooting device...".to_string(),
        FirmwareUpdateState::Verifying => "Verifying update...".to_string(),
        FirmwareUpdateState::Complete => "Update complete!".to_string(),
        FirmwareUpdateState::Failed(e) => format!("Failed: {}", e),
    };

    let is_error = matches!(current_state, FirmwareUpdateState::Failed(_));
    let is_complete = matches!(current_state, FirmwareUpdateState::Complete);
    let is_flashing = matches!(
        current_state,
        FirmwareUpdateState::Flashing { .. } | FirmwareUpdateState::FlashingNetCore { .. }
    );

    rsx! {
        div {
            class: "flex flex-col p-3 bg-gray-50 dark:bg-gray-600 rounded gap-2",
            div {
                class: "flex items-center justify-between",
                div {
                    class: "flex items-center gap-3",
                    div {
                        class: if is_error { "w-2 h-2 bg-red-500 rounded-full" }
                               else if is_complete { "w-2 h-2 bg-green-500 rounded-full" }
                               else { "w-2 h-2 bg-yellow-500 rounded-full animate-pulse" }
                    }
                    span {
                        class: "text-sm font-mono text-gray-700 dark:text-gray-200",
                        "{uuid[0]:02x}:{uuid[1]:02x}:{uuid[2]:02x}:{uuid[3]:02x}:{uuid[4]:02x}:{uuid[5]:02x}"
                    }
                }
                span {
                    class: if is_error { "text-xs text-red-500" }
                           else if is_complete { "text-xs text-green-500" }
                           else { "text-xs text-yellow-500" },
                    "{state_display}"
                }
            }
            if is_flashing {
                {
                    let (progress, total) = match &current_state {
                        FirmwareUpdateState::Flashing { progress, total }
                        | FirmwareUpdateState::FlashingNetCore { progress, total } => (*progress, *total),
                        _ => (0, 1),
                    };
                    let percent = if total > 0 { (progress * 100) / total } else { 0 };
                    rsx! {
                        div {
                            class: "w-full bg-gray-200 dark:bg-gray-700 rounded-full h-2",
                            div {
                                class: "bg-blue-600 h-2 rounded-full transition-all duration-100",
                                style: "width: {percent}%",
                            }
                        }
                        span {
                            class: "text-xs text-gray-400",
                            "{progress / 1024} / {total / 1024} KB"
                        }
                    }
                }
            }
        }
    }
}
