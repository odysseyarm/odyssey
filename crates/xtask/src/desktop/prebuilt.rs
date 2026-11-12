//! `cargo xtask desktop bundle <variant> --prebuilt <dir>`: wrap binaries that were already
//! built, instead of compiling the app and cargo-packager on the machine that runs WiX.
//!
//! `<dir>` is the `desktop-windows` output of the Nix flake (cross-built from Linux):
//!
//! ```text
//! online/odyssey-desktop.exe   default features: the app downloads the Orbbec SDK
//! full/odyssey-desktop.exe     --no-default-features: no SDK downloader
//! tools/cargo-packager.exe     the version pinned in [workspace.metadata.bin] of Cargo.toml
//! tools/xtask.exe              this program
//! packager.json                [package.metadata.packager] of crates/odyssey-desktop
//! prebuilt.json                app version, packager version and the sha256 of every file above
//! ```
//!
//! Everything the installer is built from is checked against `prebuilt.json`, which Nix wrote
//! next to the binaries, and `prebuilt.json` against the checkout: a download that was damaged
//! or came from another release is refused instead of being packaged.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use eros::Context as _;
use serde::Deserialize;

use super::{DIR, Variant, orbbec};
use crate::cmd;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: String,
    cargo_packager: String,
    /// Path under the prebuilt directory, to sha256.
    sha256: BTreeMap<String, String>,
}

/// The cargo-packager command for `variant`. cargo-packager is given its configuration
/// instead of finding it: it would otherwise ask cargo to resolve the whole workspace, which
/// needs network access and the private odyssey-cv checkout.
pub(super) fn packager(variant: Variant, dir: &Path, out_dir: &str) -> eros::Result<Command> {
    let dir = std::path::absolute(dir).with_context(|| format!("resolving {}", dir.display()))?;
    let manifest_path = dir.join("prebuilt.json");
    let manifest: Manifest = serde_json::from_slice(&orbbec::read(&manifest_path)?)
        .with_context(|| format!("parsing {}", manifest_path.display()))?;
    let app_version = std::env::var("ODYSSEY_BUILD_VERSION").unwrap_or(cmd::package_version(DIR)?);
    let pinned = pinned_packager(&root_manifest()?)?;
    let config = verify(variant, &dir, &manifest, &app_version, &pinned)?;

    let mut command = cmd::at(DIR, dir.join(PACKAGER));
    command.arg("--config").arg(config.to_string()).args([
        "--formats",
        "wix",
        "--out-dir",
        out_dir,
    ]);
    Ok(command)
}

const PACKAGER: &str = "tools/cargo-packager.exe";

/// Checks `dir` against its manifest and the checkout, and returns the cargo-packager
/// configuration for `variant` pointing at that variant's binaries.
fn verify(
    variant: Variant,
    dir: &Path,
    manifest: &Manifest,
    app_version: &str,
    pinned_packager: &str,
) -> eros::Result<serde_json::Value> {
    if manifest.version != app_version {
        eros::bail!(
            "{} holds odyssey-desktop {}, but this checkout is {app_version}",
            dir.display(),
            manifest.version
        );
    }
    if manifest.cargo_packager != pinned_packager {
        eros::bail!(
            "{} holds cargo-packager {}, but Cargo.toml pins {pinned_packager}",
            dir.display(),
            manifest.cargo_packager
        );
    }

    let binaries = dir.join(variant.name());
    let app = format!("{}/odyssey-desktop.exe", variant.name());
    let config = "packager.json";
    let mut config_json = Vec::new();
    for relative in [app.as_str(), PACKAGER, config] {
        let Some(expected) = manifest.sha256.get(relative) else {
            eros::bail!(
                "{} does not list {relative}",
                dir.join("prebuilt.json").display()
            );
        };
        let bytes = orbbec::read(&dir.join(relative))?;
        orbbec::verify(&bytes, expected, relative)?;
        if relative == config {
            config_json = bytes;
        }
    }

    let mut config: serde_json::Value =
        serde_json::from_slice(&config_json).context("parsing packager.json")?;
    let binaries = binaries
        .to_str()
        .context("the prebuilt directory is not UTF-8")?;
    config["binariesDir"] = binaries.into();
    Ok(config)
}

fn root_manifest() -> eros::Result<String> {
    let path: PathBuf = cmd::root().join("Cargo.toml");
    let bytes = orbbec::read(&path)?;
    Ok(String::from_utf8(bytes).with_context(|| format!("{} is not UTF-8", path.display()))?)
}

/// The cargo-packager version of `[workspace.metadata.bin]`.
fn pinned_packager(manifest: &str) -> eros::Result<String> {
    let version = manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("cargo-packager = {"))
        .and_then(|entry| entry.split_once("version = \""))
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(version, _)| version);
    let Some(version) = version else {
        eros::bail!("Cargo.toml pins no cargo-packager in [workspace.metadata.bin]");
    };
    Ok(version.to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use sha2::{Digest as _, Sha256};

    use super::{Manifest, Variant, pinned_packager, verify};

    const FILES: [(&str, &[u8]); 5] = [
        ("online/odyssey-desktop.exe", b"online app"),
        ("full/odyssey-desktop.exe", b"full app"),
        ("tools/cargo-packager.exe", b"packager"),
        ("tools/xtask.exe", b"xtask"),
        ("packager.json", br#"{"productName":"Odyssey"}"#),
    ];

    /// A prebuilt directory as Nix lays it out, and the manifest that describes it.
    fn fixture(name: &str) -> (PathBuf, Manifest) {
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("xtask-prebuilt-{name}-{pid}"));
        let _ = fs::remove_dir_all(&dir);
        let mut sha256 = BTreeMap::new();
        for (path, bytes) in FILES {
            let file = dir.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, bytes).unwrap();
            sha256.insert(path.to_owned(), hex::encode(Sha256::digest(bytes)));
        }
        let manifest = Manifest {
            version: "1.2.3".to_owned(),
            cargo_packager: "0.11.8".to_owned(),
            sha256,
        };
        (dir, manifest)
    }

    fn check(variant: Variant, dir: &Path, manifest: &Manifest) -> eros::Result<serde_json::Value> {
        verify(variant, dir, manifest, "1.2.3", "0.11.8")
    }

    #[test]
    fn a_consistent_directory_is_packaged_from_its_own_variant() {
        let (dir, manifest) = fixture("ok");
        let config = check(Variant::Full, &dir, &manifest).unwrap();
        assert_eq!(config["productName"], "Odyssey");
        assert_eq!(
            PathBuf::from(config["binariesDir"].as_str().unwrap()),
            dir.join("full")
        );
    }

    #[test]
    fn a_binary_that_differs_from_the_manifest_is_refused() {
        let (dir, manifest) = fixture("tampered");
        fs::write(dir.join("tools/cargo-packager.exe"), b"not the packager").unwrap();
        let error = check(Variant::Online, &dir, &manifest).unwrap_err();
        assert!(
            format!("{error:?}").contains("tools/cargo-packager.exe"),
            "{error:?}"
        );
    }

    #[test]
    fn the_online_app_cannot_stand_in_for_the_full_one() {
        let (dir, manifest) = fixture("variant");
        fs::copy(
            dir.join("online/odyssey-desktop.exe"),
            dir.join("full/odyssey-desktop.exe"),
        )
        .unwrap();
        let error = check(Variant::Full, &dir, &manifest).unwrap_err();
        assert!(
            format!("{error:?}").contains("full/odyssey-desktop.exe"),
            "{error:?}"
        );
        assert!(check(Variant::Online, &dir, &manifest).is_ok());
    }

    #[test]
    fn a_directory_from_another_release_or_packager_pin_is_refused() {
        let (dir, manifest) = fixture("release");
        assert!(verify(Variant::Online, &dir, &manifest, "1.2.4", "0.11.8").is_err());
        assert!(verify(Variant::Online, &dir, &manifest, "1.2.3", "0.12.0").is_err());
    }

    #[test]
    fn a_file_the_manifest_does_not_list_is_refused() {
        let (dir, mut manifest) = fixture("unlisted");
        manifest.sha256.remove("full/odyssey-desktop.exe");
        assert!(check(Variant::Full, &dir, &manifest).is_err());
    }

    #[test]
    fn the_pin_is_the_packager_entry_not_a_neighbouring_tool() {
        let manifest = r#"
[workspace.metadata.bin]
dioxus-cli = { version = "0.7.10", locked = true, bins = ["dx"] }
cargo-packager = { version = "0.11.8", locked = true, bins = ["cargo-packager"] }
"#;
        assert_eq!(pinned_packager(manifest).unwrap(), "0.11.8");
    }
}
