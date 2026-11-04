use std::collections::HashMap;

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::{Device, DeviceCapabilities as DC};
use odyssey_hub_server::firmware::{self, FirmwareManifest};
use slab::Slab;

use super::bootloader::ensure_bootloader;
use super::dfu_steps::{flash_with_progress, wait_for_app_mode};
use super::netcore::{flash_netcore, wait_for_netcore_release};
use super::update_check::desktop_version;
use super::update_manager::FirmwareUpdateManager;
use super::update_state::FirmwareUpdateState;

/// Start a firmware update - runs entirely on tokio runtime
pub(super) fn start_firmware_update(
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

/// Run the actual firmware update sequence
pub(super) async fn run_firmware_update(
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
    // The same reading the row used to pick the entry, so both agree.
    let firmware_entry = firmware::select_firmware(
        &manifest,
        &device_type,
        desktop_version(),
        device.netcore_release,
    )
    .offered
    .ok_or("No firmware for this device type is installable by this unit and this version of Odyssey Desktop")?;

    // The net core is released separately from the application, so the entry
    // only says which release it was tested with. Whether it has to be written
    // is decided here, before anything is downloaded or detached, from the
    // release the hub last read. A reading of 0 means unknown (an application
    // too old to report one, a net core that does not answer, a device in the
    // bootloader) and is replaced whenever a release is published.
    //
    // With the current stage2 nothing reports the net core while the device is
    // in the bootloader, and a net core update leaves the device there until an
    // application is written. So `requires_netcore` is judged against what the
    // device already runs (the entry's own net core never counts), and the net
    // core is written first and read back only after the new application runs,
    // which detects a silently failed copy as an error telling the user to run
    // the update again.
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
    let mut images: HashMap<&'static str, Vec<u8>> = HashMap::new();
    for slot_file in ["app-a", "app-b"] {
        let Some(file_entry) = firmware_entry.files.get(slot_file) else {
            continue;
        };
        tracing::info!("Downloading {} for {}", slot_file, firmware_entry.version);
        let data = firmware::download_firmware(
            &device_type,
            firmware::ReleaseSource::Application {
                version: &firmware_entry.version,
            },
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

    ensure_bootloader(
        manager,
        device_uuid,
        vid,
        pid,
        bootloader_image.as_ref(),
        firmware_entry.requires_bootloader,
    )
    .await?;

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
pub(super) async fn write_application(
    manager: &FirmwareUpdateManager,
    device_uuid: [u8; 6],
    vid: u16,
    pid: u16,
    images: &mut HashMap<&'static str, Vec<u8>>,
    bootloader_image: Option<&(Vec<u8>, u16)>,
    requires_bootloader: u16,
) -> Result<(), String> {
    ensure_bootloader(
        manager,
        device_uuid,
        vid,
        pid,
        bootloader_image,
        requires_bootloader,
    )
    .await?;

    // Determine target slot
    let (slot_name, slot_file, alt) =
        firmware::determine_target_slot_for_device(&device_uuid, vid, pid)
            .map_err(|e| format!("Failed to determine target slot: {e}"))?;

    tracing::info!(
        "Target slot: {} (alt {}), firmware file: {}",
        slot_name,
        alt,
        slot_file
    );

    let firmware_data = images
        .remove(slot_file)
        .ok_or_else(|| format!("Firmware file {slot_file} not found in manifest"))?;

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
