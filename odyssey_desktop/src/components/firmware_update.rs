//! Firmware update UI components for individual devices.
//!
//! Architecture:
//! - Firmware update state is stored in a global Arc<Mutex<HashMap>> shared between
//!   the tokio runtime and Dioxus components
//! - The actual update runs entirely on tokio::spawn, independent of Dioxus component lifecycle
//! - Dioxus components poll the shared state periodically to update the UI

use dioxus::{logger::tracing, prelude::*};
use odyssey_hub_common::device::{Device, DeviceCapabilities};
use odyssey_hub_server::firmware::{self, FirmwareManifest};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// State for a single device's firmware update
#[derive(Clone, Debug, PartialEq)]
pub enum FirmwareUpdateState {
    /// No update available or not yet checked
    Idle,
    /// Update is available
    Available {
        current: [u16; 3],
        available: String,
        device_type: String,
        vid: u16,
        pid: u16,
    },
    /// Currently downloading firmware
    Downloading,
    /// Sending DFU detach command
    Detaching,
    /// Waiting for device to enter DFU mode
    WaitingForDfu,
    /// Flashing firmware with progress (bytes_written, total_bytes)
    Flashing { progress: usize, total: usize },
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
            .unwrap()
            .get(uuid)
            .cloned()
            .unwrap_or(FirmwareUpdateState::Idle)
    }

    pub fn set_state(&self, uuid: [u8; 6], state: FirmwareUpdateState) {
        self.states.lock().unwrap().insert(uuid, state);
    }

    pub fn is_updating(&self, uuid: &[u8; 6]) -> bool {
        self.updating.lock().unwrap().contains(uuid)
    }

    pub fn add_updating(&self, uuid: [u8; 6]) {
        self.updating.lock().unwrap().insert(uuid);
    }

    pub fn remove_updating(&self, uuid: &[u8; 6]) {
        self.updating.lock().unwrap().remove(uuid);
    }

    pub fn get_all_updating(&self) -> Vec<[u8; 6]> {
        self.updating.lock().unwrap().iter().copied().collect()
    }

    pub fn clear_state(&self, uuid: &[u8; 6]) {
        self.states.lock().unwrap().remove(uuid);
        self.updating.lock().unwrap().remove(uuid);
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

    // Check if update is available when manifest or device changes
    use_effect({
        let device = device.clone();
        let manager = props.manager.clone();
        move || {
            // Only check for updates if device has USB control capability
            if !device.capabilities.contains(DeviceCapabilities::CONTROL) {
                return;
            }

            // Don't reset state if we're in the middle of an update
            let current_state = manager.get_state(&device_uuid);
            if !matches!(
                current_state,
                FirmwareUpdateState::Idle | FirmwareUpdateState::Available { .. }
            ) {
                return;
            }

            if let (Some(manifest), Some(fw_version)) =
                (manifest(), device.firmware_version.as_ref())
            {
                let device_version = [fw_version[0], fw_version[1], fw_version[2]];

                if let Some((vid, pid, device_type)) = firmware::find_any_known_device() {
                    if let Some(available_fw) =
                        firmware::find_compatible_firmware(&manifest, device_type, device_version)
                    {
                        let new_state = FirmwareUpdateState::Available {
                            current: device_version,
                            available: available_fw.version.clone(),
                            device_type: device_type.to_string(),
                            vid,
                            pid,
                        };
                        if !matches!(current_state, FirmwareUpdateState::Available { .. }) {
                            tracing::info!(
                                "Update available for {:02x?}: {}.{}.{} -> {}",
                                device_uuid,
                                device_version[0],
                                device_version[1],
                                device_version[2],
                                available_fw.version
                            );
                        }
                        manager.set_state(device_uuid, new_state);
                    }
                }
            }
        }
    });

    let state = local_state();

    match state {
        FirmwareUpdateState::Idle => rsx! {},
        FirmwareUpdateState::Available {
            available,
            device_type,
            vid,
            pid,
            ..
        } => {
            let manager = props.manager.clone();
            let manifest = manifest.clone();
            let device = device.clone();

            rsx! {
                div {
                    class: "flex items-center gap-2",
                    span {
                        class: "text-xs text-yellow-600 dark:text-yellow-400",
                        "Update available: v{available}"
                    }
                    button {
                        class: "px-2 py-1 text-xs bg-blue-600 hover:bg-blue-700 text-white rounded",
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
) {
    let device_uuid = device.uuid;

    // Mark as updating
    manager.add_updating(device_uuid);

    // Spawn the entire update on tokio - completely independent of Dioxus
    tokio::spawn(async move {
        let result = run_firmware_update(&manager, manifest, device, device_type, vid, pid).await;

        if let Err(e) = result {
            tracing::error!("Firmware update failed: {}", e);
            manager.set_state(device_uuid, FirmwareUpdateState::Failed(e));
        }

        // Clean up after a delay so user can see final state
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        manager.clear_state(&device_uuid);
    });
}

/// Run the actual firmware update sequence
async fn run_firmware_update(
    manager: &FirmwareUpdateManager,
    manifest: Option<FirmwareManifest>,
    device: Device,
    device_type: String,
    vid: u16,
    pid: u16,
) -> Result<(), String> {
    let device_uuid = device.uuid;

    let manifest = manifest.ok_or("No manifest available")?;
    let fw_version = device
        .firmware_version
        .as_ref()
        .ok_or("No firmware version")?;
    let device_version = [fw_version[0], fw_version[1], fw_version[2]];

    let firmware_entry =
        firmware::find_compatible_firmware(&manifest, &device_type, device_version)
            .ok_or("No compatible firmware found")?;

    // Send DFU detach
    tracing::info!("Sending DFU detach to {:04x}:{:04x}", vid, pid);
    manager.set_state(device_uuid, FirmwareUpdateState::Detaching);
    firmware::send_dfu_detach(vid, pid)
        .await
        .map_err(|e| format!("DFU detach failed: {}", e))?;

    // Wait for device to enter DFU mode
    tracing::info!("Waiting for device to enter DFU mode");
    manager.set_state(device_uuid, FirmwareUpdateState::WaitingForDfu);
    firmware::wait_for_dfu_device(vid, pid, std::time::Duration::from_secs(10))
        .await
        .map_err(|e| format!("Device did not enter DFU mode: {}", e))?;

    // Determine target slot
    let (slot_name, slot_file, alt) = firmware::determine_target_slot_for_device(vid, pid)
        .map_err(|e| format!("Failed to determine target slot: {}", e))?;

    tracing::info!(
        "Target slot: {} (alt {}), firmware file: {}",
        slot_name,
        alt,
        slot_file
    );

    let file_entry = firmware_entry
        .files
        .get(slot_file)
        .ok_or_else(|| format!("Firmware file {} not found in manifest", slot_file))?;

    // Download firmware
    tracing::info!("Starting firmware download for {}", slot_file);
    manager.set_state(device_uuid, FirmwareUpdateState::Downloading);
    let firmware_data = firmware::download_firmware(
        &device_type,
        &firmware_entry.version,
        slot_file,
        &file_entry.sha256,
    )
    .await
    .map_err(|e| format!("Download failed: {}", e))?;

    tracing::info!("Firmware download completed: {} bytes", firmware_data.len());

    // Flash firmware with progress updates
    tracing::info!("Flashing firmware to {}", slot_name);
    let total_size = firmware_data.len();
    manager.set_state(
        device_uuid,
        FirmwareUpdateState::Flashing {
            progress: 0,
            total: total_size,
        },
    );

    // Use atomic counter for progress
    let progress_bytes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let progress_bytes_clone = progress_bytes.clone();
    let manager_clone = manager.clone();

    // Spawn progress updater
    let progress_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let current = progress_bytes_clone.load(std::sync::atomic::Ordering::Relaxed);
            manager_clone.set_state(
                device_uuid,
                FirmwareUpdateState::Flashing {
                    progress: current,
                    total: total_size,
                },
            );
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
        vid,
        pid,
        slot_name,
        alt,
        &firmware_data,
        progress_callback,
    )
    .await;

    // Stop progress updater
    progress_task.abort();

    flash_result.map_err(|e| format!("Flash failed: {}", e))?;

    // Verify flash was complete
    let final_progress = progress_bytes.load(std::sync::atomic::Ordering::Relaxed);
    if final_progress < total_size {
        return Err(format!(
            "Flash incomplete: {}/{} bytes written",
            final_progress, total_size
        ));
    }

    // Wait for reboot
    manager.set_state(device_uuid, FirmwareUpdateState::Rebooting);
    tracing::info!("Waiting for device to reboot...");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify device reconnected
    manager.set_state(device_uuid, FirmwareUpdateState::Verifying);
    tracing::info!("Verifying firmware update...");

    let timeout = std::time::Duration::from_secs(15);
    let start = std::time::Instant::now();
    let mut reconnected = false;

    while start.elapsed() < timeout {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if firmware::is_device_in_app_mode(vid, pid) {
            tracing::info!("Device reconnected in application mode");
            reconnected = true;
            break;
        }
    }

    if reconnected {
        tracing::info!("Firmware update complete!");
    } else {
        tracing::warn!("Device did not reconnect in time, but flash appeared successful");
    }

    manager.set_state(device_uuid, FirmwareUpdateState::Complete);
    Ok(())
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
        FirmwareUpdateState::Rebooting => "Rebooting device...".to_string(),
        FirmwareUpdateState::Verifying => "Verifying update...".to_string(),
        FirmwareUpdateState::Complete => "Update complete!".to_string(),
        FirmwareUpdateState::Failed(e) => format!("Failed: {}", e),
    };

    let is_error = matches!(current_state, FirmwareUpdateState::Failed(_));
    let is_complete = matches!(current_state, FirmwareUpdateState::Complete);
    let is_flashing = matches!(current_state, FirmwareUpdateState::Flashing { .. });

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
                        FirmwareUpdateState::Flashing { progress, total } => (*progress, *total),
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
