use std::time::Duration;
use super::Link;

use odyssey_hub_common::accessory::AccessoryInfo;
use odyssey_protocol::ota::{self, Request, Status};
use crate::firmware::ota::OtaTransport;
use crate::firmware::{self, FirmwareChannel};

const APP_VERSION_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) async fn update(
    link: &mut Link,
    info: &AccessoryInfo,
    desktop_version: &semver::Version,
) -> Result<String, String> {
    let status = read_status(link).await?;
    if status.supported == 0 {
        return Err("Marker firmware does not support OTA; connect this marker by USB first".into());
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
    let known_bootloader_floor = semver::Version::new(0, 0, 1);
    if entry.requires_bootloader > known_bootloader_floor {
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
    let netcore = if let Some(file) = netcore_file
        .filter(|file| file.release.is_some_and(|release| release > status.netcore_release))
    {
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
    let app_image = entry.files.get("app").map(|file| firmware::ota::VerifiedImage {
        bytes: &app,
        manifest: file,
    });
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
            application: app_image,
            app_a: None,
            app_b: None,
            netcore: netcore_image,
            app_version,
            known_bootloader_floor: &known_bootloader_floor,
            stage2_bootloader_update: false,
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
            tokio::time::sleep(Duration::from_millis(500)).await;
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

