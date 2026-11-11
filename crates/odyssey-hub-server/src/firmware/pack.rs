//! The offline firmware pack: firmware read from a folder instead of GitHub.
//!
//! The folder is laid out exactly like what GitHub serves, so a pack is a mirror
//! and nobody has to name anything by hand:
//!
//! ```text
//! releases.stable.json                       the manifest (and releases.nightly.json)
//! atslite1-v0.1.22/app-a.bin                 an application release carries both slots
//! atslite1-v0.1.22/app-b.bin
//! atslite1-netcore-v1/net-core.bin           the net core and bootloader releases a
//! atslite1-bootloader-v4/stage2-bootloader.bin   manifest entry refers to
//! ```
//!
//! An update writes whichever application slot the unit is not running, and cannot
//! know which until it starts, so a release is only offered when **every** file its
//! manifest entry lists is in the folder at its listed size, including the single
//! application or both relocation-specific banks. A missing or half-copied file
//! offers less rather than starting an update that cannot finish. Each file is
//! checked against its manifest hash again when it is read, as a download is.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::{
    FileEntry, FirmwareChannel, FirmwareEntry, FirmwareManifest, ReleaseSource,
    SUPPORTED_MANIFEST_VERSION, verify_sha256,
};

/// A release left out of the offered ones, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub device: String,
    pub version: String,
    pub problems: Vec<String>,
}

/// A manifest read from a pack, with only the releases the folder can complete.
#[derive(Debug)]
pub struct Loaded {
    pub manifest: FirmwareManifest,
    pub dropped: Vec<Dropped>,
}

pub fn manifest_path(dir: &Path, channel: FirmwareChannel) -> PathBuf {
    dir.join(format!("releases.{}.json", channel.name()))
}

/// Where a file of a release is, relative to the pack.
fn relative_path(device: &str, source: ReleaseSource<'_>, filename: &str) -> PathBuf {
    PathBuf::from(source.tag(device)).join(super::asset_filename(device, source, filename))
}

/// Which GitHub release holds a manifest file, which is also its folder here.
///
/// `None` for a net core or bootloader entry without a release number. Older manifest
/// entries list those two for the record only: the updater never downloads one that has
/// no release, so the pack does not need it either.
fn source_of<'a>(
    entry: &'a FirmwareEntry,
    key: &str,
    file: &FileEntry,
) -> Option<ReleaseSource<'a>> {
    match key {
        "net-core" => file
            .release
            .map(|release| ReleaseSource::NetCore { release }),
        "stage2-bootloader" => file
            .release
            .map(|release| ReleaseSource::Bootloader { release }),
        _ => Some(ReleaseSource::Application {
            version: &entry.version,
        }),
    }
}

/// Everything wrong with one release in this folder.
fn problems(dir: &Path, device: &str, entry: &FirmwareEntry) -> Vec<String> {
    let mut found = Vec::new();
    if device == "dongle" {
        if !["dfu-fanstel", "dfu-laird"]
            .iter()
            .all(|key| entry.files.contains_key(*key))
        {
            found.push("the manifest must list both signed dongle packages".into());
        }
    } else if entry.application_files().is_none() {
        found.push("the manifest must list app or both app-a and app-b, not mixed images".into());
    }
    let mut keys: Vec<_> = entry.files.keys().collect();
    keys.sort();
    for key in keys {
        let file = &entry.files[key];
        let Some(source) = source_of(entry, key, file) else {
            continue;
        };
        let relative = relative_path(device, source, key);
        match std::fs::metadata(dir.join(&relative)) {
            Ok(meta) if meta.is_file() && meta.len() == file.size => {}
            Ok(meta) if meta.is_file() => found.push(format!(
                "{} is {} bytes, not the expected {} (an incomplete copy?)",
                relative.display(),
                meta.len(),
                file.size
            )),
            _ => found.push(format!("{} is missing", relative.display())),
        }
    }
    found
}

/// Reads the pack's manifest for `channel` and keeps the releases whose files are
/// all there. Fails, saying what is missing, when no release is complete.
pub fn load_manifest(
    dir: &Path,
    channel: FirmwareChannel,
) -> Result<Loaded, super::FirmwareSourceError> {
    let path = manifest_path(dir, channel);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(super::FirmwareSourceError::MissingPack {
                dir: dir.to_owned(),
                manifest: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            });
        }
        Err(source) => return Err(super::FirmwareSourceError::Read { path, source }),
    };
    let mut manifest: FirmwareManifest = serde_json::from_str(&text)
        .map_err(|source| super::FirmwareSourceError::Manifest { path, source })?;
    if manifest.version > SUPPORTED_MANIFEST_VERSION {
        return Err(super::FirmwareSourceError::ManifestVersion(
            manifest.version,
        ));
    }

    let mut dropped = Vec::new();
    for (device, device_manifest) in &mut manifest.devices {
        device_manifest.firmwares.retain(|entry| {
            let found = problems(dir, device, entry);
            if found.is_empty() {
                return true;
            }
            dropped.push(Dropped {
                device: device.clone(),
                version: entry.version.clone(),
                problems: found,
            });
            false
        });
    }
    manifest
        .devices
        .retain(|_, device| !device.firmwares.is_empty());
    dropped.sort_by(|a, b| (&a.device, &a.version).cmp(&(&b.device, &b.version)));

    if manifest.devices.is_empty() {
        // Name a few of the dropped releases so the user can see why nothing is left.
        let details = dropped
            .iter()
            .take(3)
            .map(|d| format!(" {} {}: {}.", d.device, d.version, d.problems.join("; ")))
            .collect::<Vec<_>>()
            .concat();
        return Err(super::FirmwareSourceError::IncompletePack {
            dir: dir.to_owned(),
            details,
        });
    }
    Ok(Loaded { manifest, dropped })
}

/// Reads one firmware file, bounded by its declared size, and checks its manifest hash.
pub fn read_file(
    dir: &Path,
    device: &str,
    source: ReleaseSource<'_>,
    filename: &str,
    expected_sha256: &str,
    expected_size: u64,
) -> Result<Vec<u8>, super::FirmwareSourceError> {
    let path = dir.join(relative_path(device, source, filename));
    let read = || -> std::io::Result<Vec<u8>> {
        let file = std::fs::File::open(&path)?;
        let mut data = Vec::new();
        file.take(expected_size.saturating_add(1))
            .read_to_end(&mut data)?;
        Ok(data)
    };
    let data = read().map_err(|source| super::FirmwareSourceError::Read { path, source })?;
    if data.len() as u64 != expected_size {
        return Err(super::FirmwareSourceError::ImageSize {
            expected: expected_size,
            actual: data.len() as u64,
        });
    }
    verify_sha256(&data, expected_sha256)?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use sha2::{Digest as _, Sha256};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("odyssey-pack-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(dir: &Path, relative: &str, data: &[u8]) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }

    fn sha(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    /// A manifest with one release of one device, carrying everything a real one
    /// does: both slots, a net core and a bootloader.
    fn manifest_json(version: &str) -> String {
        let file = |data: &[u8], release: Option<u16>| {
            let release = release.map_or(String::new(), |r| format!(r#", "release": {r}"#));
            format!(
                r#"{{ "sha256": "{}", "size": {}{release} }}"#,
                sha(data),
                data.len()
            )
        };
        format!(
            r#"{{ "version": 1, "devices": {{ "atslite1": {{ "firmwares": [ {{
                "version": "{version}", "requires_bootloader": 0, "requires_netcore": 0,
                "min_desktop": "0.6.0",
                "files": {{ "app-a": {}, "app-b": {}, "net-core": {}, "stage2-bootloader": {} }}
            }} ] }} }} }}"#,
            file(b"slot a", None),
            file(b"slot b", None),
            file(b"net core", Some(1)),
            file(b"boot", Some(4)),
        )
    }

    /// A pack that holds that release completely.
    fn complete_pack(name: &str) -> PathBuf {
        let dir = scratch(name);
        put(
            &dir,
            "releases.stable.json",
            manifest_json("0.2.0").as_bytes(),
        );
        put(&dir, "atslite1-v0.2.0/app-a.bin", b"slot a");
        put(&dir, "atslite1-v0.2.0/app-b.bin", b"slot b");
        put(&dir, "atslite1-netcore-v1/net-core.bin", b"net core");
        put(
            &dir,
            "atslite1-bootloader-v4/stage2-bootloader.bin",
            b"boot",
        );
        dir
    }

    fn versions(loaded: &Loaded) -> Vec<String> {
        loaded.manifest.devices["atslite1"]
            .firmwares
            .iter()
            .map(|e| e.version.clone())
            .collect()
    }

    #[test]
    fn a_complete_pack_offers_its_release() {
        let dir = complete_pack("complete");
        let loaded = load_manifest(&dir, FirmwareChannel::Stable).unwrap();
        assert_eq!(versions(&loaded), ["0.2.0"]);
        assert!(loaded.dropped.is_empty());
    }

    #[test]
    fn single_core_pack_installs_without_dual_bank_or_netcore_images() {
        let dir = scratch("single-core");
        let image = b"single application";
        let manifest = serde_json::json!({
            "version": 1,
            "devices": { "marker-control-board": { "firmwares": [{
                "version": "0.1.0", "requires_bootloader": 1, "requires_netcore": 0,
                "min_desktop": "0.7.3",
                "files": { "app": { "sha256": sha(image), "size": image.len() } }
            }] } }
        });
        put(
            &dir,
            "releases.nightly.json",
            manifest.to_string().as_bytes(),
        );
        put(&dir, "marker-control-board-v0.1.0/app.bin", image);
        let loaded = load_manifest(&dir, FirmwareChannel::Nightly).unwrap();
        let entry = &loaded.manifest.devices["marker-control-board"].firmwares[0];
        let file = &entry.files["app"];
        assert_eq!(
            read_file(
                &dir,
                "marker-control-board",
                ReleaseSource::Application {
                    version: &entry.version
                },
                "app",
                &file.sha256,
                file.size
            )
            .unwrap(),
            image
        );
    }

    #[test]
    fn dongle_pack_reads_signed_zips_and_refuses_a_missing_vendor() {
        let dir = scratch("dongle");
        let bytes = b"signed package";
        let file = serde_json::json!({"size": bytes.len(), "sha256": sha(bytes)});
        let manifest = serde_json::json!({
            "version": 1, "devices": {"dongle": {"firmwares": [{
                "version": "0.1.13", "requires_bootloader": 0, "requires_netcore": 0,
                "min_desktop": "0.7.4", "files": {"dfu-fanstel": file, "dfu-laird": file}
            }]}}
        });
        put(
            &dir,
            "releases.nightly.json",
            manifest.to_string().as_bytes(),
        );
        for vendor in ["fanstel", "laird"] {
            put(
                &dir,
                &format!("dongle-v0.1.13/dongle-fw-0.1.13-dfu-{vendor}.zip"),
                bytes,
            );
        }
        let loaded = load_manifest(&dir, FirmwareChannel::Nightly).unwrap();
        let entry = &loaded.manifest.devices["dongle"].firmwares[0];
        let file = &entry.files["dfu-fanstel"];
        assert_eq!(
            read_file(
                &dir,
                "dongle",
                ReleaseSource::Application {
                    version: &entry.version
                },
                "dfu-fanstel",
                &file.sha256,
                file.size
            )
            .unwrap(),
            bytes
        );
        std::fs::remove_file(dir.join("dongle-v0.1.13/dongle-fw-0.1.13-dfu-laird.zip")).unwrap();
        assert!(matches!(
            load_manifest(&dir, FirmwareChannel::Nightly),
            Err(super::super::FirmwareSourceError::IncompletePack { .. })
        ));
    }

    #[test]
    fn a_release_missing_either_application_slot_is_not_offered() {
        // An update picks the slot the unit is not running, so both must be there.
        for missing in ["atslite1-v0.2.0/app-a.bin", "atslite1-v0.2.0/app-b.bin"] {
            let dir = complete_pack("slot");
            std::fs::remove_file(dir.join(missing)).unwrap();
            assert!(matches!(
                load_manifest(&dir, FirmwareChannel::Stable),
                Err(super::super::FirmwareSourceError::IncompletePack { .. })
            ));
        }
    }

    #[test]
    fn a_missing_net_core_or_bootloader_also_drops_the_release() {
        for missing in [
            "atslite1-netcore-v1/net-core.bin",
            "atslite1-bootloader-v4/stage2-bootloader.bin",
        ] {
            let dir = complete_pack("support");
            std::fs::remove_file(dir.join(missing)).unwrap();
            assert!(
                load_manifest(&dir, FirmwareChannel::Stable).is_err(),
                "{missing}"
            );
        }
    }

    #[test]
    fn an_older_entry_without_net_core_or_bootloader_releases_needs_only_its_slots() {
        // Older manifest entries list a net core and bootloader with no release number;
        // the updater never downloads those, so a pack need not carry them.
        let dir = scratch("old-style");
        let without_releases = manifest_json("0.1.9")
            .replace(r#", "release": 1"#, "")
            .replace(r#", "release": 4"#, "");
        put(&dir, "releases.stable.json", without_releases.as_bytes());
        put(&dir, "atslite1-v0.1.9/app-a.bin", b"slot a");
        put(&dir, "atslite1-v0.1.9/app-b.bin", b"slot b");

        let loaded = load_manifest(&dir, FirmwareChannel::Stable).unwrap();

        assert_eq!(versions(&loaded), ["0.1.9"]);
    }

    #[test]
    fn a_half_copied_file_is_not_a_complete_one() {
        let dir = complete_pack("short");
        put(&dir, "atslite1-v0.2.0/app-b.bin", b"slot"); // cut short
        assert!(matches!(
            load_manifest(&dir, FirmwareChannel::Stable),
            Err(super::super::FirmwareSourceError::IncompletePack { .. })
        ));
    }

    #[test]
    fn only_the_incomplete_releases_are_left_out_and_they_are_reported() {
        let dir = complete_pack("mixed");
        // Add a second, newer release whose files never arrived.
        let mut manifest: serde_json::Value =
            serde_json::from_str(&manifest_json("0.2.0")).unwrap();
        let newer: serde_json::Value = serde_json::from_str(&manifest_json("0.3.0")).unwrap();
        let entry = newer["devices"]["atslite1"]["firmwares"][0].clone();
        manifest["devices"]["atslite1"]["firmwares"]
            .as_array_mut()
            .unwrap()
            .insert(0, entry);
        put(
            &dir,
            "releases.stable.json",
            manifest.to_string().as_bytes(),
        );

        let loaded = load_manifest(&dir, FirmwareChannel::Stable).unwrap();

        assert_eq!(
            versions(&loaded),
            ["0.2.0"],
            "the update that cannot finish is not offered"
        );
        assert_eq!(loaded.dropped.len(), 1);
        assert_eq!(loaded.dropped[0].version, "0.3.0");
        assert!(
            loaded.dropped[0]
                .problems
                .iter()
                .any(|p| p.contains("app-a.bin"))
        );
    }

    #[test]
    fn a_manifest_that_lists_only_one_slot_is_not_trusted() {
        let dir = complete_pack("one-slot");
        let mut manifest: serde_json::Value =
            serde_json::from_str(&manifest_json("0.2.0")).unwrap();
        manifest["devices"]["atslite1"]["firmwares"][0]["files"]
            .as_object_mut()
            .unwrap()
            .remove("app-b");
        put(
            &dir,
            "releases.stable.json",
            manifest.to_string().as_bytes(),
        );
        assert!(matches!(
            load_manifest(&dir, FirmwareChannel::Stable),
            Err(super::super::FirmwareSourceError::IncompletePack { .. })
        ));
    }

    #[test]
    fn each_channel_reads_its_own_manifest() {
        let dir = complete_pack("channels");
        assert!(
            load_manifest(&dir, FirmwareChannel::Nightly).is_err(),
            "no nightly manifest here"
        );
    }

    #[test]
    fn a_newer_manifest_than_this_build_understands_is_refused() {
        let dir = complete_pack("future");
        put(
            &dir,
            "releases.stable.json",
            manifest_json("0.2.0")
                .replace(r#""version": 1,"#, r#""version": 99,"#)
                .as_bytes(),
        );
        assert!(matches!(
            load_manifest(&dir, FirmwareChannel::Stable),
            Err(super::super::FirmwareSourceError::ManifestVersion(99))
        ));
    }

    #[test]
    fn declared_size_rejects_truncated_and_oversized_images_before_hashing() {
        let dir = complete_pack("size");
        let source = ReleaseSource::Application { version: "0.2.0" };
        for expected in [5, 7] {
            assert!(matches!(
                read_file(&dir, "atslite1", source, "app-a", &sha(b"slot a"), expected),
                Err(super::super::FirmwareSourceError::ImageSize { expected: wanted, actual: 6 })
                    if wanted == expected
            ));
        }
    }

    #[test]
    fn a_file_is_read_from_where_a_download_would_have_come_from_and_its_hash_checked() {
        let dir = complete_pack("read");
        let source = ReleaseSource::Application { version: "0.2.0" };
        let data = read_file(&dir, "atslite1", source, "app-a", &sha(b"slot a"), 6).unwrap();
        assert_eq!(data, b"slot a");

        let net = ReleaseSource::NetCore { release: 1 };
        assert!(read_file(&dir, "atslite1", net, "net-core", &sha(b"net core"), 8).is_ok());

        // The wrong content under the right name is caught.
        put(&dir, "atslite1-v0.2.0/app-a.bin", b"slot X");
        assert!(matches!(
            read_file(&dir, "atslite1", source, "app-a", &sha(b"slot a"), 6),
            Err(super::super::FirmwareSourceError::HashMismatch { .. })
        ));

        // A file that is not there is named.
        assert!(matches!(
            read_file(&dir, "atslite1", source, "app-c", &sha(b"x"), 1),
            Err(super::super::FirmwareSourceError::Read { path, source })
                if path == dir.join("atslite1-v0.2.0/app-c.bin")
                    && source.kind() == std::io::ErrorKind::NotFound
        ));
    }
}
