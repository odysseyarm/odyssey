use std::future;
use std::time::Duration;

use backon::Retryable as _;
use dioxus::logger::tracing;
use dioxus::prelude::{ReadableExt as _, SyncSignal};
use odyssey_hub_common::device::{Device, DeviceCapabilities};
use odyssey_hub_server::firmware::{self, FirmwareManifest};
use slab::Slab;

use super::dfu_session::DfuSession;
use super::downloads::{Downloads, published_release};
use super::netcore::wait_for_netcore_release;
use super::poll_window::{poll_window, ready};
use super::update_check::{desktop_version, format_version, is_in_bootloader};
use super::update_manager::FirmwareUpdateManager;
use super::update_state::FirmwareUpdateState;

/// How long the finished update stays visible before its state is cleared.
const FINAL_STATE_LINGER: Duration = Duration::from_secs(5);
const REBOOT_SETTLE_DELAY: Duration = Duration::from_millis(500);
/// How long the device gets to reappear in application mode after flashing.
const APP_MODE_TIMEOUT: Duration = Duration::from_secs(15);
/// Allow the hub's control connection to read the returning application's version.
const APP_VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const APP_VERSION_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Start a firmware update on the tokio runtime, outside any component's
/// lifetime. Does nothing when an update for the device is already running.
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

    // The update runs on tokio, independent of Dioxus. The
    // task records its own failure in the manager, which is what the UI polls;
    // nothing awaits the handle.
    tokio::spawn(async move {
        let result =
            run_firmware_update(&manager, manifest, device, device_type, vid, pid, devices).await;

        if let Err(e) = result {
            tracing::error!("Firmware update failed: {}", e);
            manager.set_state(device_uuid, FirmwareUpdateState::Failed(e));
        }

        tokio::time::sleep(FINAL_STATE_LINGER).await;
        manager.clear_state(&device_uuid);
    });
}

async fn run_firmware_update(
    manager: &FirmwareUpdateManager,
    manifest: Option<FirmwareManifest>,
    device: Device,
    device_type: String,
    vid: u16,
    pid: u16,
    devices: SyncSignal<Slab<Device>>,
) -> Result<(), String> {
    let session = DfuSession {
        manager,
        uuid: device.uuid,
        vid,
        pid,
    };

    let manifest = manifest.ok_or("No manifest available")?;
    // A device sitting in the bootloader has no CONTROL capability and reports
    // no version, so it takes whatever the manifest offers. Everything else
    // must have reported a version for the button to have been shown.
    if !is_in_bootloader(&device) && device.firmware_version == [0, 0, 0] {
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
    let expected_version = firmware_entry
        .version_parts()
        .ok_or("Firmware release has an unreadable application version")?;

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
    let published_netcore = published_release(firmware_entry, "net-core");
    let steps = firmware::plan_update_steps(
        device.netcore_release,
        published_netcore.map(|(_, release)| release),
        firmware_entry.requires_netcore,
    )
    .map_err(|e| e.to_string())?;
    tracing::info!("Update steps for {:02x?}: {:?}", session.uuid, steps);

    session.set_state(FirmwareUpdateState::Downloading);
    let mut downloads = Downloads::fetch(&device_type, firmware_entry, &steps).await?;

    session
        .ensure_bootloader(
            downloads.bootloader.as_ref(),
            firmware_entry.requires_bootloader,
        )
        .await?;

    for step in steps {
        match step {
            firmware::UpdateStep::FlashNetCore => {
                let image = downloads
                    .netcore
                    .as_ref()
                    .ok_or("The update plan flashes the net core but no image was downloaded")?;
                session.flash_netcore(&image.data, image.release).await?;
            }
            firmware::UpdateStep::VerifyNetCore => {
                let image = downloads
                    .netcore
                    .as_ref()
                    .ok_or("The update plan verifies the net core but none was written")?;
                session.set_state(FirmwareUpdateState::VerifyingNetCore);
                wait_for_netcore_release(&devices, &session.uuid, image.release).await?;
            }
            firmware::UpdateStep::WriteApplication => {
                session
                    .write_application(
                        &mut downloads,
                        firmware_entry.requires_bootloader,
                        &devices,
                        expected_version,
                    )
                    .await?;
            }
        }
    }

    tracing::info!("Firmware update complete!");
    session.set_state(FirmwareUpdateState::Complete);
    Ok(())
}

impl DfuSession<'_> {
    /// Detach into the bootloader if needed, update the bootloader when the release
    /// calls for it, write the application to the inactive bank, and wait for the
    /// device to come back running it.
    async fn write_application(
        &self,
        downloads: &mut Downloads,
        requires_bootloader: u16,
        devices: &SyncSignal<Slab<Device>>,
        expected_version: [u16; 3],
    ) -> Result<(), String> {
        self.ensure_bootloader(downloads.bootloader.as_ref(), requires_bootloader)
            .await?;

        let (slot_name, slot_file, alt) =
            firmware::determine_target_slot_for_device(&self.uuid, self.vid, self.pid)
                .map_err(|e| format!("Failed to determine target slot: {e}"))?;

        tracing::info!(
            "Target slot: {} (alt {}), firmware file: {}",
            slot_name,
            alt,
            slot_file
        );

        let firmware_data = downloads
            .applications
            .remove(slot_file)
            .ok_or_else(|| format!("Firmware file {slot_file} not found in manifest"))?;

        tracing::info!("Flashing firmware to {}", slot_name);
        self.flash_with_progress(
            slot_name,
            alt,
            &firmware_data,
            "Flash",
            |progress, total| FirmwareUpdateState::Flashing { progress, total },
        )
        .await?;

        self.set_state(FirmwareUpdateState::Rebooting);
        tracing::info!("Waiting for device to reboot...");
        tokio::time::sleep(REBOOT_SETTLE_DELAY).await;

        self.set_state(FirmwareUpdateState::Verifying);
        tracing::info!("Verifying firmware update...");

        if self.wait_for_app_mode(APP_MODE_TIMEOUT).await {
            // Enumeration alone also succeeds when the bootloader rejected the
            // new image and fell back to the old application.
            let mut last_version = None;
            let reported = (|| {
                last_version = reported_application_version(&devices.peek(), &self.uuid);
                future::ready(ready(last_version == Some(expected_version)))
            })
            .retry(poll_window(APP_VERSION_TIMEOUT, APP_VERSION_POLL_INTERVAL))
            .await;
            if reported.is_ok() {
                tracing::info!(
                    "Device reconnected running application {}",
                    format_version(expected_version)
                );
                return Ok(());
            }
            Err(format!(
                "Device returned but did not report firmware {} (last reading: {}). \
                 Leave it connected and try again.",
                format_version(expected_version),
                last_version
                    .map(format_version)
                    .unwrap_or_else(|| "unknown".to_string())
            ))
        } else {
            // The byte count above only proves the host finished feeding the
            // writer, so a device that never comes back is the one signal left
            // that the new image is not running.
            Err(format!(
                "Device did not reappear within {}s after flashing. It may still be \
                 in the bootloader — leave it connected and try again.",
                APP_MODE_TIMEOUT.as_secs()
            ))
        }
    }
}

/// Only the requested unit's live control connection can verify its application.
fn reported_application_version(devices: &Slab<Device>, uuid: &[u8; 6]) -> Option<[u16; 3]> {
    devices
        .iter()
        .find(|(_, device)| {
            &device.uuid == uuid && device.capabilities.contains(DeviceCapabilities::CONTROL)
        })
        .map(|(_, device)| device.firmware_version)
}
