use std::future;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use backon::Retryable as _;
use eros::Context as _;
use futures::FutureExt as _;
use odyssey_hub_client::client::{Client, DeviceClient};
use odyssey_hub_common::device::{Device, DeviceCapabilities, Transport};
use odyssey_hub_common::firmware_version::FirmwareVersion;
use odyssey_hub_server::firmware::ota::{self as host_ota, Images, LinkMtu, OtaTransport, VerifiedImage};
use odyssey_hub_server::firmware::{self, FirmwareManifest};

use super::dfu_session::DfuSession;
use super::downloads::{Downloads, published_release};
use super::netcore::wait_for_netcore_release;
use super::poll_window::{poll_window, ready};
use super::update_check::{desktop_version, format_version, is_in_bootloader};
use super::update_manager::FirmwareUpdateManager;
use super::update_state::FirmwareUpdateState;
use crate::runtime;

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
    client: Option<Client>,
) {
    let device_uuid = device.uuid;

    // Two tasks driving DFU on one device would interleave their control
    // transfers and corrupt the download.
    let Some(claim) = manager.try_claim(device_uuid) else {
        tracing::warn!("Firmware update already running for {:02x?}", device_uuid);
        return;
    };

    // Keep both the transfer and its claim outside any component's lifetime.
    // Catch panics here because nobody awaits the detached task's handle.
    runtime::spawn(async move {
        let _claim = claim;
        let result = AssertUnwindSafe(run_firmware_update(
            &manager,
            manifest,
            device,
            device_type,
            vid,
            pid,
            client,
        ))
        .catch_unwind()
        .await
        .unwrap_or_else(|_| {
            Err(eros::error!(
                "The firmware update task stopped unexpectedly"
            ))
        });

        if let Err(e) = result {
            tracing::error!("Firmware update failed: {e:?}");
            manager.set_state(
                device_uuid,
                FirmwareUpdateState::Failed(crate::error_message(&e)),
            );
        }
    });
}

async fn run_firmware_update(
    manager: &FirmwareUpdateManager,
    manifest: Option<FirmwareManifest>,
    device: Device,
    device_type: String,
    vid: u16,
    pid: u16,
    client: Option<Client>,
) -> eros::Result<()> {
    let session = DfuSession {
        manager,
        uuid: device.uuid,
        vid,
        pid,
    };

    let manifest = manifest.ok_or_else(|| eros::error!("No manifest available"))?;
    // A device sitting in the bootloader has no CONTROL capability and reports
    // no version, so it takes whatever the manifest offers. Everything else
    // must have reported a version for the button to have been shown.
    if !is_in_bootloader(&device)
        && device.firmware_version == [0, 0, 0]
        && device.firmware_identity.known().is_none()
    {
        eros::bail!("No firmware version");
    }
    // The same reading the row used to pick the entry, so both agree.
    let firmware_entry = firmware::select_firmware(
        &manifest,
        &device_type,
        &desktop_version(),
        device.netcore_release,
    )
    .offered
    .ok_or_else(|| eros::error!("No firmware for this device type is installable by this unit and this version of Odyssey Desktop"))?;
    let expected: FirmwareVersion = firmware_entry.version.parse().map_err(|error| {
        eros::error!("Firmware release has an unreadable application version: {error}")
    })?;

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
    // device already runs (the entry's own net core never counts). The net core
    // is written first and read back only after the new application runs. A
    // silently failed copy then shows up as an error telling the user to run
    // the update again.
    let published_netcore = published_release(firmware_entry, "net-core");
    let steps = firmware::plan_update_steps(
        device.netcore_release,
        published_netcore.map(|(_, release)| release),
        firmware_entry.requires_netcore,
    )?;
    tracing::info!("Update steps for {:02x?}: {:?}", session.uuid, steps);

    session.set_state(FirmwareUpdateState::Downloading);
    let mut downloads = Downloads::fetch(
        &device_type,
        firmware_entry,
        &steps,
        device.transport == Transport::UsbMux,
    )
    .await?;
    if device.transport == Transport::UsbMux {
        let client = client.ok_or_else(|| eros::error!("Hub client is disconnected"))?;
        return run_radio_ota(manager, client, device, firmware_entry, &downloads).await;
    }

    let verified_bootloader = session
        .ensure_bootloader(
            downloads.bootloader.as_ref(),
            &firmware_entry.requires_bootloader,
        )
        .await?;

    for step in steps {
        match step {
            firmware::UpdateStep::FlashNetCore => {
                let image = downloads.netcore.as_ref().ok_or_else(|| {
                    eros::error!("The update plan flashes the net core but no image was downloaded")
                })?;
                session.flash_netcore(&image.data, image.release).await?;
            }
            firmware::UpdateStep::VerifyNetCore => {
                let image = downloads.netcore.as_ref().ok_or_else(|| {
                    eros::error!("The update plan verifies the net core but none was written")
                })?;
                session.set_state(FirmwareUpdateState::VerifyingNetCore);
                wait_for_netcore_release(manager, session.uuid, image.release).await?;
            }
            firmware::UpdateStep::WriteApplication => {
                session
                    .write_application(
                        &mut downloads,
                        &firmware_entry.requires_bootloader,
                        expected,
                    )
                    .await?;
            }
        }
    }

    if let Some(bootloader_version) =
        verified_bootloader.filter(|version| *version >= semver::Version::new(0, 2, 0))
    {
        if matches!(device_type.as_str(), "atslite1" | "legacy-atslite1")
            && let Some(installed) = manager.devices().into_iter().find(|d| d.uuid == device.uuid)
            && installed.firmware_identity.known() == Some(expected)
            && super::update_check::installed_bootloader_floor(
                &manifest,
                &device_type,
                &installed,
            )
            .is_some_and(|floor| *floor >= semver::Version::new(0, 2, 0))
        {
            manager.record_radio_ota_proof(device.uuid, expected, bootloader_version);
        }
    }
    tracing::info!("Firmware update complete!");
    session.set_state(FirmwareUpdateState::Complete);
    Ok(())
}

async fn run_radio_ota(
    manager: &FirmwareUpdateManager,
    client: Client,
    device: Device,
    entry: &firmware::FirmwareEntry,
    downloads: &Downloads,
) -> eros::Result<()> {
    let known_bootloader_floor = manager
        .radio_ota_bootloader_version(
            device.uuid,
            device.firmware_identity.known(),
            device.firmware_version,
        )
        .ok_or_else(|| {
            eros::error!("No verified USB bootloader proof for this installed app; connect USB first")
        })?;
    let app_version = firmware::parse_version(&entry.version)
        .ok_or_else(|| eros::error!("Firmware application version is invalid"))?;
    let application = |name: &'static str| {
        downloads
            .applications
            .get(name)
            .zip(entry.files.get(name))
            .map(|(bytes, file)| VerifiedImage {
                bytes,
                manifest: file,
            })
    };
    let netcore = downloads
        .netcore
        .as_ref()
        .zip(entry.files.get("net-core"))
        .map(|(image, file)| VerifiedImage {
            bytes: &image.data,
            manifest: file,
        });
    let mut transport = RadioOtaTransport {
        client: client.clone(),
        device: client.device(device.clone()),
        uuid: device.uuid,
    };
    host_ota::send(
        &mut transport,
        LinkMtu::Vendor,
        entry,
        Images {
            application: application("app"),
            app_a: application("app-a"),
            app_b: application("app-b"),
            netcore,
            app_version,
            known_bootloader_floor: &known_bootloader_floor,
            stage2_bootloader_update: false,
        },
    )
    .await?;
    manager.set_state(device.uuid, FirmwareUpdateState::Complete);
    Ok(())
}

struct RadioOtaTransport {
    client: Client,
    device: DeviceClient,
    uuid: [u8; 6],
}

impl OtaTransport for RadioOtaTransport {
    async fn exchange(
        &mut self,
        frame: &[u8],
    ) -> Result<[u8; odyssey_protocol::ota::STATUS_LEN], String> {
        self.device
            .exchange_ota(frame.to_vec())
            .await
            .map_err(|error| error.to_string())
    }

    async fn verify_reconnected(
        &mut self,
        app_version: [u32; 3],
        netcore_release: u16,
    ) -> Result<bool, String> {
        let deadline = tokio::time::Instant::now() + APP_VERSION_TIMEOUT;
        loop {
            if let Ok(devices) = self.client.get_device_list().await
                && devices.iter().any(|device| {
                    device.uuid == self.uuid
                        && device.firmware_version == app_version
                        && device.netcore_release == netcore_release
                })
            {
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(APP_VERSION_POLL_INTERVAL).await;
        }
    }
}

impl DfuSession<'_> {
    /// Detach into the bootloader if needed, update the bootloader when the release
    /// calls for it, write the application to the inactive bank, and wait for the
    /// device to come back running it.
    async fn write_application(
        &self,
        downloads: &mut Downloads,
        requires_bootloader: &semver::Version,
        expected: FirmwareVersion,
    ) -> eros::Result<()> {
        self.ensure_bootloader(downloads.bootloader.as_ref(), requires_bootloader)
            .await?;

        let (slot_name, slot_file, alt) =
            firmware::determine_target_slot_for_device(&self.uuid, self.vid, self.pid)
                .context("Failed to determine target slot")?;

        tracing::info!(
            "Target slot: {} (alt {}), firmware file: {}",
            slot_name,
            alt,
            slot_file
        );

        let firmware_data = downloads
            .applications
            .remove(slot_file)
            .ok_or_else(|| eros::error!("Firmware file {slot_file} not found in manifest"))?;

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
                last_version = reported_application_version(&self.manager.devices(), self.uuid);
                future::ready(ready(last_version.as_ref().is_some_and(
                    |(core, identity)| {
                        *core == expected.core
                            && identity.is_none_or(|reported| reported == expected)
                    },
                )))
            })
            .retry(poll_window(APP_VERSION_TIMEOUT, APP_VERSION_POLL_INTERVAL))
            .await;
            if reported.is_ok() {
                tracing::info!("Device reconnected running application {expected}");
                return Ok(());
            }
            Err(eros::error!(
                "Device returned but did not report firmware {expected} (last reading: {}). \
                 Leave it connected and try again.",
                last_version.as_ref().map_or_else(
                    || "unknown".to_string(),
                    |(core, identity)| identity
                        .map_or_else(|| format_version(*core), |version| version.to_string())
                )
            ))
        } else {
            // The byte count above only proves the host finished feeding the
            // writer, so a device that never comes back is the one signal left
            // that the new image is not running.
            Err(eros::error!(
                "Device did not reappear within {}s after flashing. It may still be \
                 in the bootloader — leave it connected and try again.",
                APP_MODE_TIMEOUT.as_secs()
            ))
        }
    }
}

/// Verify the requested unit's live control connection or DFU-only USB runtime descriptor.
fn reported_application_version(
    devices: &[Device],
    uuid: [u8; 6],
) -> Option<([u32; 3], Option<FirmwareVersion>)> {
    devices
        .iter()
        .find(|device| {
            device.uuid == uuid
                && (device.capabilities.contains(DeviceCapabilities::CONTROL)
                    || device
                        .capabilities
                        .contains(DeviceCapabilities::DFU_RUNTIME_ONLY))
        })
        .map(|device| (device.firmware_version, device.firmware_identity.known()))
}
