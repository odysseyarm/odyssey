use std::collections::HashMap;

use dioxus::logger::tracing;
use odyssey_hub_server::firmware::{self, FileEntry, FirmwareEntry, UpdateStep};

/// An image whose release is versioned separately from the application.
pub(super) struct ReleaseImage {
    pub(super) data: Vec<u8>,
    pub(super) release: u16,
}

/// Everything an update writes, fetched before the device is touched.
pub(super) struct Downloads {
    pub(super) netcore: Option<ReleaseImage>,
    /// Application images by manifest file name (`app-a`, `app-b`).
    pub(super) applications: HashMap<&'static str, Vec<u8>>,
    pub(super) bootloader: Option<ReleaseImage>,
}

/// The file of `entry` named `name` together with the release it carries, for
/// the files (`net-core`, `stage2-bootloader`) that are versioned on their own.
pub(super) fn published_release<'a>(
    entry: &'a FirmwareEntry,
    name: &str,
) -> Option<(&'a FileEntry, u16)> {
    let file = entry.files.get(name)?;
    Some((file, file.release?))
}

async fn download_image(
    device_type: &str,
    source: firmware::ReleaseSource<'_>,
    name: &str,
    file: &FileEntry,
) -> Result<Vec<u8>, String> {
    firmware::download_firmware(device_type, source, name, &file.sha256)
        .await
        .map_err(|e| format!("Download of {name} failed: {e}"))
}

impl Downloads {
    /// Which bank to write is only readable once the bootloader is running, so
    /// fetch both candidates up front. Detaching is the point of no return: the
    /// app marks the boot state for DFU before resetting, and only a completed
    /// download clears that mark, so a download that fails after the detach
    /// leaves the device in the bootloader across every later reboot. The same
    /// holds for the net core image, so every download happens before the first
    /// detach.
    pub(super) async fn fetch(
        device_type: &str,
        entry: &FirmwareEntry,
        steps: &[UpdateStep],
    ) -> Result<Self, String> {
        let netcore = match published_release(entry, "net-core") {
            Some((file, release)) if steps.contains(&UpdateStep::FlashNetCore) => {
                tracing::info!("Downloading net-core release {}", release);
                let data = download_image(
                    device_type,
                    firmware::ReleaseSource::NetCore { release },
                    "net-core",
                    file,
                )
                .await?;
                Some(ReleaseImage { data, release })
            }
            _ => None,
        };

        let mut applications = HashMap::new();
        for slot_file in ["app-a", "app-b"] {
            let Some(file) = entry.files.get(slot_file) else {
                continue;
            };
            tracing::info!("Downloading {} for {}", slot_file, entry.version);
            let data = download_image(
                device_type,
                firmware::ReleaseSource::Application {
                    version: &entry.version,
                },
                slot_file,
                file,
            )
            .await?;
            applications.insert(slot_file, data);
        }
        if applications.is_empty() {
            return Err("Firmware release has no app-a or app-b image".to_string());
        }

        // The stage2 bootloader is versioned separately and republished unchanged
        // across application releases, so it is only worth downloading when the
        // release says which bootloader release it carries.
        let bootloader = match published_release(entry, "stage2-bootloader") {
            Some((file, release)) => {
                tracing::info!("Release carries stage2 bootloader release {}", release);
                let data = download_image(
                    device_type,
                    firmware::ReleaseSource::Bootloader { release },
                    "stage2-bootloader",
                    file,
                )
                .await?;
                Some(ReleaseImage { data, release })
            }
            None => None,
        };

        Ok(Self {
            netcore,
            applications,
            bootloader,
        })
    }
}
