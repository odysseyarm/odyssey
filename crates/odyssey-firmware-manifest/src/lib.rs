//! Typed view of the firmware channel manifest and the release asset naming.
//!
//! The manifest is published JSON with one list of releases per device type.
//! `odyssey_hub_server::firmware` selects and downloads from it and the desktop
//! update UI reads it through the same types, so a missing required field is
//! rejected in one place. Unknown fields are ignored on read; a caller that
//! rewrites the manifest keeps the source JSON.
//!
//! The release flow that produces entries is described in
//! `firmware/manifests/README.md`.

use std::collections::HashMap;

use serde::Deserialize;

/// Firmware channel manifest. Deserialization enforces required fields while callers may retain
/// the original JSON to preserve metadata this version does not interpret.
#[derive(Debug, Clone, Deserialize)]
pub struct FirmwareManifest {
    pub version: u32,
    pub devices: HashMap<String, DeviceManifest>,
}

/// Releases of one device type, newest first.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceManifest {
    pub firmwares: Vec<FirmwareEntry>,
}

/// One firmware release and what a unit needs before it can install it.
///
/// The floors are the compatibility claims; `firmware/manifests/README.md`
/// says how each is set. For ATS Lite, `requires_bootloader` is the floor that
/// gates Bluetooth updates; see `RADIO_OTA_MIN_BOOTLOADER` in odyssey-desktop's
/// `firmware_update/update_manager.rs`.
#[derive(Debug, Clone, Deserialize)]
pub struct FirmwareEntry {
    pub version: String,
    /// Lowest bootloader this application runs on. `0.0.0` means no floor.
    pub requires_bootloader: semver::Version,
    /// Net-core release the unit must already be running. `0` means none.
    pub requires_netcore: u16,
    /// Oldest Odyssey Desktop that may install this release.
    pub min_desktop: String,
    /// Assets by role: `app`, `app-a`/`app-b`, `net-core`, `stage2-bootloader`,
    /// and the dongle's `dfu-*` packages.
    pub files: HashMap<String, FileEntry>,
    /// Publication timestamp, when the entry has one. Hosts do not read it.
    #[serde(default)]
    pub released: Option<String>,
}

/// A release asset, identified by its SHA-256 and size. Downloads are accepted
/// only if both match.
#[derive(Debug, Clone, Deserialize)]
pub struct FileEntry {
    pub sha256: String,
    pub size: u64,
    /// Version of a bootloader asset.
    #[serde(default)]
    pub version: Option<semver::Version>,
    /// Counter of a net-core asset, which is released separately from the
    /// application and reported by the unit as its net-core release.
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

    /// The application images this release carries: a lone `app` for a
    /// single-image device, or the `app-a`/`app-b` pair for a dual-bank one.
    /// `None` for any other combination, including an `app` next to a net core
    /// or a stage2 bootloader.
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

/// Release asset file name for `key`. Every asset is `<key>.bin` except the
/// dongle's DFU packages, which carry the application version.
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
        serde_json::from_value::<FirmwareManifest>(serde_json::json!({
            "version": 2,
            "devices": { "unit": { "firmwares": [{ "version": "1.2.3", "files": {} }] } }
        }))
        .unwrap_err();
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
