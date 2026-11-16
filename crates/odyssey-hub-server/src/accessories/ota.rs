//! Over-the-air update of a marker control board.
//!
//! This is the marker's half of the radio OTA flow: it picks the firmware
//! entry, downloads and hashes the images, and hands them to
//! [`crate::firmware::ota::send`] through [`MarkerOtaTransport`]. `send` is the
//! same session ATS Lite uses; only the transport differs (see that module for
//! the comparison).
//!
//! [`Link`] is already connected to the marker over whichever radio the user
//! picked, the host's own adapter or an Odyssey dongle; `accessories/transport.rs`
//! hides the difference. The marker needs no USB bootstrap: it has one
//! bootloader, never updated over the radio, so [`MARKER_BOOTLOADER`] is a
//! constant rather than a version the unit reports, as ATS Lite does.

use std::time::Duration;

use odyssey_hub_common::accessory::AccessoryInfo;
use odyssey_protocol::ota::{self, Request, Status};

use super::Link;
use crate::firmware::ota::OtaTransport;
use crate::firmware::{self, FirmwareChannel};

/// How long the marker gets to reboot after the commit and reconnect
/// reporting the new versions.
const APP_VERSION_TIMEOUT: Duration = Duration::from_secs(30);
const RECONNECT_RETRY_DELAY: Duration = Duration::from_millis(500);

/// The marker's bootloader version. Updating it needs SWD, and `cargo xtask
/// firmware` only publishes marker entries that require exactly this version.
const MARKER_BOOTLOADER: semver::Version = semver::Version::new(0, 0, 1);

/// Updates the connected marker to the firmware the manifest offers it.
///
/// Errors are user-facing text.
pub(super) async fn update(
    link: &mut Link,
    info: &AccessoryInfo,
    desktop_version: &semver::Version,
) -> Result<String, String> {
    let status = read_status(link).await?;
    if status.supported == 0 {
        return Err(
            "Marker firmware does not support OTA; connect this marker by USB first".into(),
        );
    }

    let channel = FirmwareChannel::load().await;
    let manifest = firmware::fetch_manifest(channel)
        .await
        .map_err(|error| format!("Could not load firmware manifest: {error}"))?;
    let entry = firmware::select_firmware(
        &manifest,
        "marker-control-board",
        desktop_version,
        status.netcore_release,
    )
    .offered
    .ok_or_else(|| "No marker firmware is installable; connect this marker by USB".to_string())?
    .clone();
    if entry.requires_bootloader > MARKER_BOOTLOADER {
        return Err(format!(
            "Marker firmware requires bootloader {}; connect this marker by USB",
            entry.requires_bootloader
        ));
    }

    let app_file = entry
        .files
        .get("app")
        .ok_or_else(|| "Marker manifest is missing app image".to_string())?;
    let app = firmware::download_firmware(
        "marker-control-board",
        firmware::ReleaseSource::Application {
            version: &entry.version,
        },
        "app",
        &app_file.sha256,
        app_file.size,
    )
    .await
    .map_err(|error| format!("Could not download verified marker app: {error}"))?;

    let netcore_file = entry.files.get("net-core");
    let netcore = if let Some(file) = netcore_file.filter(|file| {
        file.release
            .is_some_and(|release| release > status.netcore_release)
    }) {
        Some(
            firmware::download_firmware(
                "marker-control-board",
                firmware::ReleaseSource::Application {
                    version: &entry.version,
                },
                "net-core",
                &file.sha256,
                file.size,
            )
            .await
            .map_err(|error| format!("Could not download verified marker net-core: {error}"))?,
        )
    } else {
        None
    };
    let app_version = firmware::parse_version(&entry.version)
        .ok_or_else(|| "Marker app version is invalid".to_string())?;
    let app_image = firmware::ota::VerifiedImage {
        bytes: &app,
        manifest: app_file,
    };
    let netcore_image = netcore
        .as_ref()
        .zip(netcore_file)
        .map(|(bytes, manifest)| firmware::ota::VerifiedImage { bytes, manifest });
    let mut transport = MarkerOtaTransport { link, info };
    firmware::ota::send(
        &mut transport,
        firmware::ota::LinkMtu::Marker,
        &entry,
        firmware::ota::Images {
            application: Some(app_image),
            app_a: None,
            app_b: None,
            netcore: netcore_image,
            app_version,
            known_bootloader_floor: &MARKER_BOOTLOADER,
        },
    )
    .await
    .map_err(|error| format!("{error}; USB recovery required for this same marker"))?;
    Ok(format!("Marker updated to {}", entry.version))
}

struct MarkerOtaTransport<'a> {
    link: &'a mut Link,
    info: &'a AccessoryInfo,
}

impl OtaTransport for MarkerOtaTransport<'_> {
    fn exchange(
        &mut self,
        frame: &[u8],
    ) -> impl Future<Output = Result<[u8; ota::STATUS_LEN], String>> {
        self.link.marker_ota_exchange(frame)
    }

    async fn verify_reconnected(
        &mut self,
        app_version: [u32; 3],
        netcore_release: u16,
    ) -> Result<bool, String> {
        let deadline = tokio::time::Instant::now() + APP_VERSION_TIMEOUT;
        loop {
            let _ = self.link.disconnect(false).await;
            tokio::time::sleep(RECONNECT_RETRY_DELAY).await;
            if self.link.connect(self.info, false).await.is_ok()
                && let Ok(status) = read_status(self.link).await
                && status.firmware_version == app_version
                && status.netcore_release == netcore_release
            {
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }
        }
    }
}

async fn read_status(link: &mut Link) -> Result<Status, String> {
    let mut frame = [0; ota::MAX_FRAME];
    let len = Request::Query
        .encode(&mut frame)
        .map_err(|error| error.to_string())?;
    let bytes = link.marker_ota_exchange(&frame[..len]).await?;
    Status::decode(&bytes).map_err(|error| error.to_string())
}
