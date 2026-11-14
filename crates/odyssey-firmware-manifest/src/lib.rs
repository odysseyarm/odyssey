use std::collections::HashMap;

use serde::Deserialize;

/// Firmware channel manifest. Deserialization enforces required fields while callers may retain
/// the original JSON to preserve metadata this version does not interpret.
#[derive(Debug, Clone, Deserialize)]
pub struct FirmwareManifest {
    pub version: u32,
    pub devices: HashMap<String, DeviceManifest>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceManifest {
    pub firmwares: Vec<FirmwareEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FirmwareEntry {
    pub version: String,
    pub requires_bootloader: semver::Version,
    pub requires_netcore: u16,
    pub min_desktop: String,
    pub files: HashMap<String, FileEntry>,
    #[serde(default)]
    pub released: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileEntry {
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub version: Option<semver::Version>,
    #[serde(default)]
    pub release: Option<u16>,
}

impl FirmwareEntry {
    pub fn version_parts(&self) -> Option<[u32; 3]> {
        let version = semver::Version::parse(&self.version).ok()?;
        Some([
            u32::try_from(version.major).ok()?,
            u32::try_from(version.minor).ok()?,
            u32::try_from(version.patch).ok()?,
        ])
    }

    pub fn application_files(&self) -> Option<impl Iterator<Item = (&'static str, &FileEntry)>> {
        let files = match (
            self.files.get("app"),
            self.files.get("app-a"),
            self.files.get("app-b"),
        ) {
            (Some(app), None, None)
                if self.requires_netcore == 0
                    && !self.files.contains_key("net-core")
                    && !self.files.contains_key("stage2-bootloader") =>
            {
                [Some(("app", app)), None]
            }
            (None, Some(a), Some(b)) => [Some(("app-a", a)), Some(("app-b", b))],
            _ => return None,
        };
        Some(files.into_iter().flatten())
    }
}

pub fn asset_filename(device: &str, source: ReleaseSource<'_>, key: &str) -> String {
    match (device, source, key) {
        ("dongle", ReleaseSource::Application { version }, "dfu-fanstel" | "dfu-laird") => {
            format!("dongle-fw-{version}-{key}.zip")
        }
        _ => format!("{key}.bin"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseSource<'a> {
    Application { version: &'a str },
    Bootloader { version: &'a semver::Version },
    NetCore { release: u16 },
}

impl ReleaseSource<'_> {
    /// Preserve historical counter tags for shipped bootloaders.
    pub fn tag(&self, device_type: &str) -> String {
        match self {
            Self::Application { version } => format!("{device_type}-v{version}"),
            Self::Bootloader { version } => {
                let legacy = if version.pre.is_empty() && version.build.is_empty() {
                    match (
                        u32::try_from(version.major),
                        u32::try_from(version.minor),
                        u32::try_from(version.patch),
                    ) {
                        (Ok(major), Ok(minor), Ok(patch)) => {
                            odyssey_version::legacy_bootloader_counter([major, minor, patch])
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                match legacy {
                    Some(counter) => format!("{device_type}-bootloader-v{counter}"),
                    None => format!("{device_type}-bootloader-v{version}"),
                }
            }
            Self::NetCore { release } => format!("{device_type}-netcore-v{release}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FirmwareEntry, FirmwareManifest, ReleaseSource, asset_filename};

    #[test]
    fn typed_manifest_requires_update_floors_and_accepts_unknown_metadata() {
        let manifest: FirmwareManifest = serde_json::from_value(serde_json::json!({
            "version": 2,
            "future_metadata": true,
            "devices": { "unit": { "firmwares": [{
                "version": "1.2.3",
                "requires_bootloader": "0.0.0",
                "requires_netcore": 0,
                "min_desktop": "0.1.0",
                "files": { "app": { "sha256": "abc", "size": 1, "future": "kept by source JSON" } }
            }] } }
        }))
        .expect("valid manifest retains forward-compatible parsing");
        assert_eq!(manifest.version, 2);
        assert!(
            serde_json::from_value::<FirmwareManifest>(serde_json::json!({
                "version": 2,
                "devices": { "unit": { "firmwares": [{ "version": "1.2.3", "files": {} }] } }
            }))
            .is_err()
        );
    }

    #[test]
    fn release_tags_share_application_netcore_and_bootloader_alias_rules() {
        assert_eq!(
            ReleaseSource::Application { version: "1.2.3" }.tag("unit"),
            "unit-v1.2.3"
        );
        assert_eq!(
            ReleaseSource::NetCore { release: 2 }.tag("unit"),
            "unit-netcore-v2"
        );
        let version = semver::Version::parse("0.0.4").expect("valid semver");
        assert_eq!(
            ReleaseSource::Bootloader { version: &version }.tag("unit"),
            "unit-bootloader-v4"
        );
        let version = semver::Version::parse("0.0.4-rc.1").expect("valid semver");
        assert_eq!(
            ReleaseSource::Bootloader { version: &version }.tag("unit"),
            "unit-bootloader-v0.0.4-rc.1"
        );
    }

    #[test]
    fn moved_firmware_helpers_keep_version_layout_and_asset_rules() {
        let entry: FirmwareEntry = serde_json::from_value(serde_json::json!({
            "version": "1.2.3",
            "requires_bootloader": "0.0.0",
            "requires_netcore": 0,
            "min_desktop": "0.1.0",
            "files": {
                "app-a": { "sha256": "a", "size": 1 },
                "app-b": { "sha256": "b", "size": 1 }
            }
        }))
        .expect("valid firmware entry");
        assert_eq!(entry.version_parts(), Some([1, 2, 3]));
        let files: Vec<_> = entry
            .application_files()
            .expect("dual-bank images")
            .map(|(key, _)| key)
            .collect();
        assert_eq!(files, ["app-a", "app-b"]);
        assert_eq!(
            asset_filename(
                "dongle",
                ReleaseSource::Application { version: "1.2.3" },
                "dfu-laird"
            ),
            "dongle-fw-1.2.3-dfu-laird.zip"
        );
    }
}
