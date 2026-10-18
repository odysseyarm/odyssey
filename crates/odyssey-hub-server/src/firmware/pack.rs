//! The offline firmware pack: firmware read from a folder instead of GitHub.
//!
//! The folder is laid out exactly like what GitHub serves, so a pack is a mirror
//! and nobody has to name anything by hand:
//!
//! ```text
//! releases.stable.json                       the manifest (and releases.alpha.json)
//! atslite1-v0.1.22/app-a.bin                 an application release carries both slots
//! atslite1-v0.1.22/app-b.bin
//! atslite1-netcore-v1/net-core.bin           the net core and bootloader releases a
//! atslite1-bootloader-v4/stage2-bootloader.bin   manifest entry refers to
//! ```
//!
//! An update writes whichever application slot the unit is not running, and cannot
//! know which until it starts, so a release is only offered when **every** file its
//! manifest entry lists is in the folder at its listed size, both application slots
//! included. A pack that is missing a file, or holds a half-copied one, therefore
//! offers less rather than starting an update that cannot finish. Each file is
//! checked against its manifest hash again when it is read, as a download is.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{
    FileEntry, FirmwareChannel, FirmwareEntry, FirmwareManifest, ReleaseSource,
    SUPPORTED_MANIFEST_VERSION, verify_sha256,
};

/// The application slots an update chooses between.
const REQUIRED_FILES: [&str; 2] = ["app-a", "app-b"];

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
    PathBuf::from(source.tag(device)).join(format!("{filename}.bin"))
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
    for required in REQUIRED_FILES {
        if !entry.files.contains_key(required) {
            found.push(format!("the manifest lists no {required}"));
        }
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
pub fn load_manifest(dir: &Path, channel: FirmwareChannel) -> Result<Loaded> {
    let path = manifest_path(dir, channel);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
            "There is no firmware pack in {}: {} is missing. Unpack the Odyssey offline \
             firmware pack into that folder.",
            dir.display(),
            path.file_name().unwrap_or_default().to_string_lossy()
        ),
        Err(e) => return Err(e).with_context(|| format!("could not read {}", path.display())),
    };
    let mut manifest: FirmwareManifest = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a firmware manifest", path.display()))?;
    if manifest.version > SUPPORTED_MANIFEST_VERSION {
        bail!(
            "the firmware pack's manifest is version {}, newer than the {} this build \
             understands; update Odyssey",
            manifest.version,
            SUPPORTED_MANIFEST_VERSION
        );
    }

    let mut dropped = Vec::new();
    for (device, device_manifest) in manifest.devices.iter_mut() {
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
        let mut message = format!(
            "The firmware pack in {} has no complete release.",
            dir.display()
        );
        for d in dropped.iter().take(3) {
            message.push_str(&format!(
                " {} {}: {}.",
                d.device,
                d.version,
                d.problems.join("; ")
            ));
        }
        bail!(message);
    }
    Ok(Loaded { manifest, dropped })
}

/// Reads one firmware file from the pack and checks it against its manifest hash.
pub fn read_file(
    dir: &Path,
    device: &str,
    source: ReleaseSource<'_>,
    filename: &str,
    expected_sha256: &str,
) -> Result<Vec<u8>> {
    let path = dir.join(relative_path(device, source, filename));
    let data = std::fs::read(&path)
        .with_context(|| format!("the firmware pack is missing {}", path.display()))?;
    verify_sha256(&data, expected_sha256)?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

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
    fn a_release_missing_either_application_slot_is_not_offered() {
        // An update picks the slot the unit is not running, so both must be there.
        for missing in ["atslite1-v0.2.0/app-a.bin", "atslite1-v0.2.0/app-b.bin"] {
            let dir = complete_pack("slot");
            std::fs::remove_file(dir.join(missing)).unwrap();
            let err = load_manifest(&dir, FirmwareChannel::Stable)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains(&missing.replace('/', std::path::MAIN_SEPARATOR_STR))
                    || err.contains(missing),
                "{err}"
            );
            assert!(err.contains("no complete release"), "{err}");
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
        let err = load_manifest(&dir, FirmwareChannel::Stable)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not the expected 6"), "{err}");
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
        let err = load_manifest(&dir, FirmwareChannel::Stable)
            .unwrap_err()
            .to_string();
        assert!(err.contains("lists no app-b"), "{err}");
    }

    #[test]
    fn a_folder_with_no_pack_says_what_to_do() {
        let dir = scratch("empty");
        let err = load_manifest(&dir, FirmwareChannel::Stable)
            .unwrap_err()
            .to_string();
        assert!(err.contains("There is no firmware pack"), "{err}");
        assert!(err.contains("releases.stable.json"), "{err}");
        assert!(err.contains(&dir.display().to_string()), "{err}");
    }

    #[test]
    fn each_channel_reads_its_own_manifest() {
        let dir = complete_pack("channels");
        assert!(
            load_manifest(&dir, FirmwareChannel::Alpha).is_err(),
            "no alpha manifest here"
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
        let err = load_manifest(&dir, FirmwareChannel::Stable)
            .unwrap_err()
            .to_string();
        assert!(err.contains("version 99"), "{err}");
    }

    #[test]
    fn a_file_is_read_from_where_a_download_would_have_come_from_and_its_hash_checked() {
        let dir = complete_pack("read");
        let source = ReleaseSource::Application { version: "0.2.0" };
        let data = read_file(&dir, "atslite1", source, "app-a", &sha(b"slot a")).unwrap();
        assert_eq!(data, b"slot a");

        let net = ReleaseSource::NetCore { release: 1 };
        assert!(read_file(&dir, "atslite1", net, "net-core", &sha(b"net core")).is_ok());

        // The wrong content under the right name is caught.
        put(&dir, "atslite1-v0.2.0/app-a.bin", b"slot X");
        let err = read_file(&dir, "atslite1", source, "app-a", &sha(b"slot a"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("SHA256 mismatch"), "{err}");

        // A file that is not there is named.
        let gone = read_file(&dir, "atslite1", source, "app-c", &sha(b"x"))
            .unwrap_err()
            .to_string();
        assert!(gone.contains("app-c.bin"), "{gone}");
    }
}
