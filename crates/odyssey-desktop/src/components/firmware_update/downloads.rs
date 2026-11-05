use std::collections::HashMap;

use eros::Context as _;
use odyssey_hub_server::firmware::{self, FileEntry, FirmwareEntry, UpdateStep};

/// An image whose release is versioned separately from the application.
pub(super) struct ReleaseImage {
    pub(super) data: Vec<u8>,
    pub(super) release: u16,
}

/// Everything an update writes, fetched before the device is touched.
pub(super) struct Downloads {
    pub(super) netcore: Option<ReleaseImage>,
    /// Application images by manifest file name (`app` or `app-a` and `app-b`).
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
) -> eros::Result<Vec<u8>> {
    let data = firmware::download_firmware(device_type, source, name, &file.sha256, file.size)
        .await
        .with_context(|| format!("Download of {name} failed"))?;
    validate_image_size(name, data.len(), file.size)?;
    Ok(data)
}

/// A matching hash does not make an empty image flashable or reconcile a bad
/// manifest size. Reject both before any update can detach the device.
fn validate_image_size(name: &str, actual: usize, expected: u64) -> eros::Result<()> {
    if actual == 0 {
        eros::bail!("Firmware image {name} is empty");
    }
    if actual as u64 != expected {
        eros::bail!(
            "Firmware image {name} has {actual} bytes, but the manifest declares {expected}"
        );
    }
    Ok(())
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
    ) -> eros::Result<Self> {
        let application_files = entry.application_files().ok_or_else(|| {
            eros::error!(
                "Firmware release must contain app or both app-a and app-b, not mixed images"
            )
        })?;
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
        for (slot_file, file) in application_files {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_mis_sized_images_cannot_reach_the_device() {
        assert!(validate_image_size("app-a", 0, 0).is_err());
        assert!(validate_image_size("app-a", 2, 3).is_err());
        assert!(validate_image_size("app-a", 4, 3).is_err());
        assert!(validate_image_size("app-a", 3, 3).is_ok());
    }

    #[test]
    fn missing_bank_is_not_installable() {
        for present in ["app-a", "app-b"] {
            let entry = FirmwareEntry {
                version: "1.0.0".into(),
                requires_bootloader: 0,
                requires_netcore: 0,
                min_desktop: "0.6.0".into(),
                files: HashMap::from([(
                    present.into(),
                    FileEntry {
                        sha256: "unused".into(),
                        size: 1,
                        release: None,
                    },
                )]),
                released: None,
            };
            assert!(
                entry.application_files().is_none(),
                "A release with only {present} cannot cover both possible inactive banks"
            );
        }
    }
}
