//! The Orbbec camera SDK on disk: where the app looks for it, and in the online
//! build how it gets there.
//!
//! The SDK is optional. It is a native library the automatic calibration wizard
//! loads at run time (see `odyssey_autocalib::orbbec::runtime`), and it can come
//! from two places:
//!
//! * the offline installer, which carries it in `orbbec-sdk\` beside the app, or
//! * a download the user asked for, kept in `orbbec-sdk\` under the app's data
//!   folder. Only builds with the `sdk-download` feature contain that code.
//!
//! The version, files and hashes both paths use are in `orbbec-sdk.json`, which
//! the Windows release workflow reads too, so there is one place to bump.

use std::path::{Path, PathBuf};

use odyssey_autocalib::orbbec::LIBRARY_NAME;

use crate::paths;

/// Directory name of an SDK copy, under the app (offline installer) or under the
/// app's data folder (download).
pub const DIR_NAME: &str = "orbbec-sdk";

/// The pinned SDK: only the download and its tests need to know it. The offline
/// build finds the SDK by directory alone.
#[cfg(any(feature = "sdk-download", test))]
mod pin_file {
    use std::sync::LazyLock;

    use serde::Deserialize;

    /// The SDK this build is tested with.
    const PIN_JSON: &str = include_str!("../orbbec-sdk.json");

    #[derive(Debug, Deserialize)]
    pub struct Pin {
        pub version: String,
        pub eula: Remote,
        pub windows_x64: Archive,
    }

    #[derive(Debug, Deserialize)]
    pub struct Remote {
        pub url: String,
        pub sha256: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct Archive {
        pub url: String,
        pub sha256: String,
        pub size: u64,
        /// Paths inside the archive, as `/`-separated names.
        pub files: Vec<String>,
    }

    pub fn pin() -> &'static Pin {
        static PIN: LazyLock<Pin> = LazyLock::new(|| {
            serde_json::from_str(PIN_JSON).expect("orbbec-sdk.json is valid; a test parses it")
        });
        &PIN
    }

    /// Where a file of the archive lands, relative to the SDK directory: its place
    /// in the archive without the leading `bin/`, so the library sits beside its
    /// `extensions/` folder, which is the layout the loader expects.
    pub fn install_path(archive_path: &str) -> &str {
        archive_path.strip_prefix("bin/").unwrap_or(archive_path)
    }
}

#[cfg(any(feature = "sdk-download", test))]
pub use pin_file::{install_path, pin};

/// The directory the offline installer puts the SDK in.
fn bundled_dir() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join(DIR_NAME))
}

/// The directory a download is kept in.
pub fn managed_dir() -> Option<PathBuf> {
    paths::orbbec_sdk_dir()
}

fn holds_library(dir: &Path) -> bool {
    dir.join(LIBRARY_NAME).is_file()
}

/// The directory to hand the loader as the application's own copy, if there is
/// one. The installer's copy wins over a download.
pub fn search_dir() -> Option<PathBuf> {
    [bundled_dir(), managed_dir()]
        .into_iter()
        .flatten()
        .find(|dir| holds_library(dir))
}

/// Whether a download of ours is on disk, for the "Remove" action.
pub fn has_managed_copy() -> bool {
    managed_dir().is_some_and(|dir| dir.exists())
}

/// How Setup offers a missing SDK on this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The app downloads it after the user accepts the licence.
    Download,
    /// This build never fetches software; the installer carries it as an option.
    Installer,
    /// Nothing is automated here: point at Orbbec's download page.
    Link,
}

pub fn delivery() -> Delivery {
    let windows = cfg!(all(windows, target_arch = "x86_64"));
    if !windows {
        Delivery::Link
    } else if cfg!(feature = "sdk-download") && managed_dir().is_some() {
        Delivery::Download
    } else {
        Delivery::Installer
    }
}

/// How a removal ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    Removed,
    /// Windows would not delete the library, because the running app has it
    /// loaded for good (see `runtime.rs`); it is deleted the next time the app
    /// starts, by [`finish_pending_removal`].
    AfterRestart,
}

fn removal_marker(dir: &Path) -> PathBuf {
    dir.with_file_name(format!("{DIR_NAME}.remove"))
}

fn remove_at(dir: &Path) -> std::io::Result<Removal> {
    let marker = removal_marker(dir);
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {
            let _ = std::fs::remove_file(&marker);
            Ok(Removal::Removed)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = std::fs::remove_file(&marker);
            Ok(Removal::Removed)
        }
        Err(_) => {
            std::fs::write(&marker, b"")?;
            Ok(Removal::AfterRestart)
        }
    }
}

/// Deletes the downloaded copy, or arranges for it to be deleted at the next
/// start when it is in use.
pub fn remove_managed_copy() -> std::io::Result<Removal> {
    match managed_dir() {
        Some(dir) => remove_at(&dir),
        None => Ok(Removal::Removed),
    }
}

/// Whether a removal is waiting for the next start.
pub fn removal_pending() -> bool {
    managed_dir().is_some_and(|dir| removal_marker(&dir).exists())
}

/// Completes a removal that had to wait for a restart. Call it at startup,
/// before anything can load the library.
pub fn finish_pending_removal() {
    let Some(dir) = managed_dir() else { return };
    if removal_marker(&dir).exists() {
        if let Err(e) = remove_at(&dir) {
            dioxus::logger::tracing::warn!(
                "Could not finish removing the downloaded Orbbec SDK: {e}"
            );
        }
    }
}

#[cfg(feature = "sdk-download")]
pub use download::{Phase, install};

#[cfg(feature = "sdk-download")]
mod download {
    use std::io::{Cursor, Read};
    use std::path::Path;

    use anyhow::{Context, Result, bail};
    use sha2::{Digest, Sha256};

    use super::{DIR_NAME, install_path, managed_dir, pin};

    /// Largest file taken out of the archive. The real ones are a few MB; this
    /// only stops a corrupt or hostile archive from filling the disk.
    const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

    /// What the download is doing, for a progress display.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Phase {
        Downloading { received: u64, total: u64 },
        Verifying,
        Installing,
    }

    /// Downloads, verifies and unpacks the pinned SDK into the app's data folder
    /// and returns its directory. The previous copy is replaced only once the new
    /// one is complete.
    pub async fn install(mut progress: impl FnMut(Phase)) -> Result<std::path::PathBuf> {
        let dest = managed_dir().context("this system has no app data folder")?;
        let pin = pin();

        let archive = fetch(&pin.windows_x64.url, pin.windows_x64.size, &mut progress).await?;
        progress(Phase::Verifying);
        verify(&archive, &pin.windows_x64.sha256).context("the Orbbec SDK download is corrupt")?;

        let eula = fetch(&pin.eula.url, 0, &mut |_| {}).await?;
        verify(&eula, &pin.eula.sha256).context("the Orbbec licence download is corrupt")?;

        progress(Phase::Installing);
        let staging = dest.with_file_name(format!("{DIR_NAME}.new"));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        let unpacked = extract(&archive, &pin.windows_x64.files, &staging)
            .and_then(|()| std::fs::write(staging.join("EULA.txt"), &eula).map_err(Into::into));
        if let Err(e) = unpacked {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
        replace_dir(&staging, &dest)?;
        Ok(dest)
    }

    async fn fetch(
        url: &str,
        expected_size: u64,
        progress: &mut impl FnMut(Phase),
    ) -> Result<Vec<u8>> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("odyssey-desktop/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let mut response = client
            .get(url)
            .send()
            .await
            .with_context(|| format!("could not reach {url}"))?
            .error_for_status()?;
        let total = response.content_length().unwrap_or(expected_size);
        let mut bytes = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
        while let Some(chunk) = response.chunk().await? {
            bytes.extend_from_slice(&chunk);
            progress(Phase::Downloading {
                received: bytes.len() as u64,
                total,
            });
        }
        Ok(bytes)
    }

    /// Fails unless `bytes` hash to `expected` (lowercase or uppercase hex).
    pub(super) fn verify(bytes: &[u8], expected: &str) -> Result<()> {
        let actual = hex::encode(Sha256::digest(bytes));
        if actual.eq_ignore_ascii_case(expected) {
            Ok(())
        } else {
            bail!("expected sha256 {expected}, got {actual}")
        }
    }

    /// Writes the listed files of a zip archive under `dest`, flattening
    /// `bin/`. Only names on the list are read, so nothing in the archive can
    /// place a file elsewhere. Archives made on Windows store `\` separators.
    pub(super) fn extract(archive: &[u8], files: &[String], dest: &Path) -> Result<()> {
        let mut zip = zip::ZipArchive::new(Cursor::new(archive)).context("not a zip archive")?;
        let mut index = std::collections::HashMap::new();
        for i in 0..zip.len() {
            let name = zip.name_for_index(i).unwrap_or_default().replace('\\', "/");
            index.insert(name, i);
        }

        let missing: Vec<&str> = files
            .iter()
            .map(String::as_str)
            .filter(|f| !index.contains_key(*f))
            .collect();
        if !missing.is_empty() {
            bail!(
                "the archive does not contain {}; this app expects Orbbec SDK {}",
                missing.join(", "),
                pin().version
            );
        }

        for file in files {
            let mut entry = zip.by_index(index[file.as_str()])?;
            if entry.size() > MAX_FILE_BYTES {
                bail!("{file} is larger than expected");
            }
            let mut contents = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut contents)?;
            let target = dest.join(install_path(file));
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, contents)
                .with_context(|| format!("could not write {}", target.display()))?;
        }
        Ok(())
    }

    /// Puts `staging` where `dest` is, removing what was there.
    fn replace_dir(staging: &Path, dest: &Path) -> Result<()> {
        match std::fs::remove_dir_all(dest) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(e).with_context(|| {
                    format!(
                        "could not replace {}; close anything using the camera and try again",
                        dest.display()
                    )
                });
            }
            _ => {}
        }
        std::fs::rename(staging, dest)?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use std::io::Write;

        use super::*;

        /// A zip laid out like Orbbec's: Windows `\` separators, a `bin\` folder.
        fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
            let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for (name, data) in entries {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap().into_inner()
        }

        fn names(list: &[&str]) -> Vec<String> {
            list.iter().map(|s| s.to_string()).collect()
        }

        fn tempdir(name: &str) -> std::path::PathBuf {
            let dir = std::env::temp_dir()
                .join(format!("odyssey-sdk-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        #[test]
        fn library_lands_beside_its_extensions() {
            let zip = archive(&[
                ("bin\\OrbbecSDK.dll", b"dll"),
                ("bin\\extensions\\filters\\f.dll", b"ext"),
                ("LICENSE.txt", b"licence"),
                ("examples\\src\\main.cpp", b"not wanted"),
            ]);
            let dest = tempdir("layout");
            extract(
                &zip,
                &names(&[
                    "bin/OrbbecSDK.dll",
                    "bin/extensions/filters/f.dll",
                    "LICENSE.txt",
                ]),
                &dest,
            )
            .unwrap();

            assert_eq!(std::fs::read(dest.join("OrbbecSDK.dll")).unwrap(), b"dll");
            assert_eq!(
                std::fs::read(dest.join("extensions/filters/f.dll")).unwrap(),
                b"ext"
            );
            assert_eq!(std::fs::read(dest.join("LICENSE.txt")).unwrap(), b"licence");
            assert!(
                !dest.join("examples").exists(),
                "unlisted files are not unpacked"
            );
        }

        #[test]
        fn an_archive_missing_a_listed_file_is_refused_with_its_name() {
            let zip = archive(&[("bin\\OrbbecSDK.dll", b"dll")]);
            let dest = tempdir("missing");
            let err =
                extract(&zip, &names(&["bin/OrbbecSDK.dll", "bin/gone.dll"]), &dest).unwrap_err();
            let message = err.to_string();
            assert!(message.contains("bin/gone.dll"), "{message}");
            assert!(!message.contains("bin/OrbbecSDK.dll"), "{message}");
        }

        #[test]
        fn nothing_outside_the_list_can_be_written() {
            // An entry that tries to climb out of the folder is never opened: only
            // the names on the list are.
            let zip = archive(&[
                ("bin\\OrbbecSDK.dll", b"dll"),
                ("..\\..\\evil.dll", b"evil"),
            ]);
            let dest = tempdir("slip");
            extract(&zip, &names(&["bin/OrbbecSDK.dll"]), &dest).unwrap();
            assert!(!dest.parent().unwrap().join("evil.dll").exists());
        }

        #[test]
        fn a_wrong_hash_is_refused_and_a_right_one_accepted_in_either_case() {
            let hash = hex::encode(Sha256::digest(b"abc"));
            assert!(verify(b"abc", &hash).is_ok());
            assert!(verify(b"abc", &hash.to_uppercase()).is_ok());
            assert!(verify(b"abd", &hash).is_err());
        }

        #[test]
        fn replacing_a_copy_swaps_it_whole() {
            let root = tempdir("replace");
            let dest = root.join("orbbec-sdk");
            std::fs::create_dir_all(&dest).unwrap();
            std::fs::write(dest.join("old.dll"), b"old").unwrap();
            let staging = root.join("orbbec-sdk.new");
            std::fs::create_dir_all(&staging).unwrap();
            std::fs::write(staging.join("OrbbecSDK.dll"), b"new").unwrap();

            replace_dir(&staging, &dest).unwrap();

            assert!(dest.join("OrbbecSDK.dll").is_file());
            assert!(
                !dest.join("old.dll").exists(),
                "nothing of the old copy survives"
            );
            assert!(!staging.exists());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pin_is_complete_and_installs_the_library_where_the_loader_looks() {
        let pin = pin();
        assert!(!pin.version.is_empty());
        for hash in [&pin.eula.sha256, &pin.windows_x64.sha256] {
            assert_eq!(hash.len(), 64, "sha256 is 64 hex digits: {hash}");
            assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{hash}");
        }
        assert!(
            pin.windows_x64.url.contains(&pin.version),
            "the archive is the pinned version"
        );
        assert!(
            pin.eula.url.contains(&pin.version),
            "the licence is the pinned version's"
        );

        let installed: Vec<&str> = pin
            .windows_x64
            .files
            .iter()
            .map(|f| install_path(f))
            .collect();
        assert!(installed.contains(&"OrbbecSDK.dll"));
        assert!(
            installed.iter().any(|p| p.starts_with("extensions/")),
            "the library needs its extensions folder beside it"
        );
        let unique: std::collections::HashSet<_> = installed.iter().collect();
        assert_eq!(
            unique.len(),
            installed.len(),
            "two archive files would overwrite each other"
        );
        assert!(
            installed
                .iter()
                .all(|p| !p.contains('\\') && !p.starts_with('/') && !p.contains(".."))
        );
    }

    #[test]
    fn the_pinned_library_name_is_what_the_loader_loads_on_windows() {
        if cfg!(windows) {
            assert_eq!(LIBRARY_NAME, "OrbbecSDK.dll");
            assert!(
                pin()
                    .windows_x64
                    .files
                    .iter()
                    .any(|f| install_path(f) == LIBRARY_NAME)
            );
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("odyssey-sdk-removal-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn removing_a_copy_deletes_it_and_leaves_no_marker() {
        let root = scratch("remove");
        let dir = root.join(DIR_NAME);
        std::fs::create_dir_all(dir.join("extensions")).unwrap();
        std::fs::write(dir.join("OrbbecSDK.dll"), b"x").unwrap();
        std::fs::write(removal_marker(&dir), b"").unwrap(); // left by an earlier attempt

        assert_eq!(remove_at(&dir).unwrap(), Removal::Removed);

        assert!(!dir.exists());
        assert!(!removal_marker(&dir).exists());
    }

    #[test]
    fn removing_a_copy_that_is_already_gone_is_not_an_error() {
        let root = scratch("gone");
        assert_eq!(remove_at(&root.join(DIR_NAME)).unwrap(), Removal::Removed);
    }

    #[test]
    fn a_copy_that_cannot_be_deleted_is_marked_for_the_next_start() {
        // A file where the directory should be: remove_dir_all fails, which stands in
        // for Windows refusing to delete a library that is loaded.
        let root = scratch("locked");
        let dir = root.join(DIR_NAME);
        std::fs::write(&dir, b"not a directory").unwrap();
        let marker = removal_marker(&dir);

        assert_eq!(remove_at(&dir).unwrap(), Removal::AfterRestart);
        assert!(marker.exists(), "the next start finishes the job");

        // The next start, with the thing no longer in the way, completes it.
        std::fs::remove_file(&dir).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(remove_at(&dir).unwrap(), Removal::Removed);
        assert!(!dir.exists() && !marker.exists());
    }
}
