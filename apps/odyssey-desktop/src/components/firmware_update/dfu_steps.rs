use std::sync::Arc;

use dioxus::logger::tracing;
use odyssey_hub_server::firmware;

use super::update_manager::FirmwareUpdateManager;
use super::update_state::FirmwareUpdateState;

/// Poll until the device enumerates in application mode, `false` on timeout.
pub(super) async fn wait_for_app_mode(
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
pub(super) async fn enter_dfu(
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
        tracing::info!(
            "Device {:02x?} is already in DFU mode, skipping detach",
            device_uuid
        );
        return Ok(());
    }

    // Send DFU detach
    tracing::info!(
        "Sending DFU detach to {:04x}:{:04x} (uuid {:02x?})",
        vid,
        pid,
        device_uuid
    );
    manager.set_state(device_uuid, FirmwareUpdateState::Detaching);
    firmware::send_dfu_detach(&device_uuid, vid, pid)
        .await
        .map_err(|e| format!("DFU detach failed: {e}"))?;

    // Wait for device to enter DFU mode
    tracing::info!("Waiting for device to enter DFU mode");
    manager.set_state(device_uuid, FirmwareUpdateState::WaitingForDfu);
    firmware::wait_for_dfu_device(&device_uuid, vid, pid, std::time::Duration::from_secs(10))
        .await
        .map_err(|e| format!("Device did not enter DFU mode: {e}"))
}

/// Write one image to a DFU alt setting, publishing progress as it goes.
pub(super) async fn flash_with_progress(
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
pub(super) async fn wait_for_bootloader_reset(
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

/// Wait for a device that has just reset to come back and put it in DFU mode,
/// whichever way it returns: still in the bootloader, or running the
/// application, which then gets detached again. A bootloader that finishes its
/// work (swapping itself in, copying the net core) boots the application unless
/// something asks for DFU, so the unit has to be taken back in rather than
/// waited for.
pub(super) async fn return_to_dfu(
    manager: &FirmwareUpdateManager,
    device_uuid: [u8; 6],
    vid: u16,
    pid: u16,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        match enter_dfu(manager, device_uuid, vid, pid).await {
            Ok(()) => return Ok(()),
            Err(e) if std::time::Instant::now() < deadline => {
                tracing::debug!("Device not back yet: {e}");
            }
            Err(e) => return Err(e),
        }
    }
}
