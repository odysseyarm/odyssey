//! The Orbbec camera SDK on disk: where the app looks for it, whether the copy it
//! found is the one this build is tested with, and in the online build how a copy
//! gets there.
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
//!
//! Once the library is loaded it stays mapped until the app exits, and Windows will
//! not rename or delete files that are mapped. Replacing or removing a copy that is
//! in use therefore moves it aside in one step, which either works or changes
//! nothing, and otherwise leaves a marker for the next start to act on.

use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};

use odyssey_autocalib::orbbec::{LIBRARY_NAME, RuntimeStatus, SdkVersion};

use crate::paths;

/// Directory name of an SDK copy, under the app (offline installer) or under the
/// app's data folder (download).
pub const DIR_NAME: &str = "orbbec-sdk";

/// The SDK this build is tested with.
mod pin_file {
    use std::sync::LazyLock;

    use odyssey_autocalib::orbbec::SdkVersion;
    use serde::Deserialize;

    const PIN_JSON: &str = include_str!("../orbbec-sdk.json");

    // Only the online build downloads, so only it reads the URLs, hashes and files.
    #[derive(Debug, Deserialize)]
    pub struct Pin {
        pub version: String,
        #[cfg_attr(not(feature = "sdk-download"), allow(dead_code))]
        pub eula: Remote,
        #[cfg_attr(not(feature = "sdk-download"), allow(dead_code))]
        pub windows_x64: Archive,
    }

    #[derive(Debug, Deserialize)]
    #[cfg_attr(not(feature = "sdk-download"), allow(dead_code))]
    pub struct Remote {
        pub url: String,
        pub sha256: String,
    }

    #[derive(Debug, Deserialize)]
    #[cfg_attr(not(feature = "sdk-download"), allow(dead_code))]
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

    /// The pinned version as the loader reports versions.
    pub fn pinned_version() -> SdkVersion {
        let mut parts = pin().version.split('.').map(|part| {
            part.parse::<u32>()
                .expect("the pinned version is major.minor.patch; a test parses it")
        });
        let mut next = || parts.next().expect("the pinned version has three parts");
        SdkVersion {
            major: next(),
            minor: next(),
            patch: next(),
        }
    }

    /// Where a file of the archive lands, relative to the SDK directory: its place
    /// in the archive without the leading `bin/`, so the library sits beside its
    /// `extensions/` folder, which is the layout the loader expects.
    #[cfg_attr(not(any(feature = "sdk-download", test)), allow(dead_code))]
    pub fn install_path(archive_path: &str) -> &str {
        archive_path.strip_prefix("bin/").unwrap_or(archive_path)
    }
}

#[cfg(any(feature = "sdk-download", test))]
pub use pin_file::install_path;
pub use pin_file::{pin, pinned_version};

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
    /// This build never downloads the camera SDK; the installer carries it as an option.
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

// ------------------------------------------------------------ Versions --

/// What to tell the user about a working SDK that is not the version this build
/// is tested with. A library the loader accepts (2.8 or newer) can still differ
/// from the pin; it is only a warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Advisory {
    /// Older than the pin. `replaceable` is set when the running copy is one this
    /// app downloaded, which a new download replaces; a copy found elsewhere is
    /// the user's to update.
    Older {
        found: SdkVersion,
        pinned: SdkVersion,
        replaceable: bool,
    },
    /// Newer than the pin. There is nothing to do about it but to know.
    Newer {
        found: SdkVersion,
        pinned: SdkVersion,
    },
}

/// Compares the running SDK with `pinned`. `managed_dir` is where this app keeps
/// its downloads, to tell its own copy from one found elsewhere.
pub fn advisory(
    status: &RuntimeStatus,
    managed_dir: Option<&Path>,
    pinned: SdkVersion,
) -> Option<Advisory> {
    let RuntimeStatus::Available {
        version,
        library_path,
    } = status
    else {
        return None;
    };
    match version.cmp(&pinned) {
        Ordering::Equal => None,
        Ordering::Greater => Some(Advisory::Newer {
            found: *version,
            pinned,
        }),
        Ordering::Less => Some(Advisory::Older {
            found: *version,
            pinned,
            // A library the system loader found by name has no path, and is not ours.
            replaceable: library_path
                .as_deref()
                .zip(managed_dir)
                .is_some_and(|(library, dir)| library.starts_with(dir)),
        }),
    }
}

/// [`advisory`] for the SDK the app is running. Call it once cameras have been
/// listed: the library is then loaded and cached, so this is cheap.
pub fn current_advisory() -> Option<Advisory> {
    let status = odyssey_autocalib::orbbec::runtime_status(search_dir().as_deref());
    advisory(&status, managed_dir().as_deref(), pinned_version())
}

// ------------------------------------------------- Replace and remove --

/// How a removal ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    Removed,
    /// The copy is in use, so nothing was touched; it is deleted the next time the
    /// app starts, by [`finish_pending`].
    AfterRestart,
}

/// Renames a directory; a parameter so tests can stand in for a mapped library
/// that Windows will not let go of.
type Rename<'a> = &'a dyn Fn(&Path, &Path) -> io::Result<()>;

fn rename_fs(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

fn sibling(dir: &Path, suffix: &str) -> PathBuf {
    dir.with_file_name(format!("{DIR_NAME}.{suffix}"))
}

/// Where a finished download waits when the copy it replaces is in use.
fn staging_dir(dir: &Path) -> PathBuf {
    sibling(dir, "new")
}

/// Written into a staged copy once it is complete, so a download interrupted
/// halfway is never swapped in.
const STAGED_OK: &str = ".staged";

fn removal_marker(dir: &Path) -> PathBuf {
    sibling(dir, "remove")
}

/// Moves `dir` out of the way. One rename: it works or nothing changes, unlike
/// deleting files one by one, which could strip a running SDK of its extensions
/// before reaching the library Windows will not delete.
fn move_aside(dir: &Path, rename: Rename<'_>) -> io::Result<Option<PathBuf>> {
    if !dir.exists() {
        return Ok(None);
    }
    let old = sibling(dir, "old");
    let _ = std::fs::remove_dir_all(&old);
    rename(dir, &old)?;
    Ok(Some(old))
}

/// Puts the finished copy in `staging` where `dest` is, keeping the old one if
/// anything fails.
pub(crate) fn swap_in(staging: &Path, dest: &Path, rename: Rename<'_>) -> io::Result<()> {
    let old = move_aside(dest, rename)?;
    if let Err(e) = rename(staging, dest) {
        if let Some(old) = &old {
            let _ = rename(old, dest);
        }
        return Err(e);
    }
    let _ = std::fs::remove_file(dest.join(STAGED_OK));
    if let Some(old) = old {
        let _ = std::fs::remove_dir_all(old);
    }
    Ok(())
}

fn remove_at_with(dir: &Path, rename: Rename<'_>) -> io::Result<Removal> {
    let marker = removal_marker(dir);
    match move_aside(dir, rename) {
        Ok(old) => {
            if let Some(old) = old {
                let _ = std::fs::remove_dir_all(old);
            }
            let _ = std::fs::remove_dir_all(staging_dir(dir));
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
pub fn remove_managed_copy() -> io::Result<Removal> {
    match managed_dir() {
        Some(dir) => remove_at_with(&dir, &rename_fs),
        None => Ok(Removal::Removed),
    }
}

/// Whether a removal is waiting for the next start.
pub fn removal_pending() -> bool {
    managed_dir().is_some_and(|dir| removal_marker(&dir).exists())
}

/// Whether a downloaded replacement is waiting for the next start.
pub fn update_pending() -> bool {
    managed_dir().is_some_and(|dir| staging_dir(&dir).join(STAGED_OK).is_file())
}

fn finish_pending_at(dir: &Path, rename: Rename<'_>) {
    // A removal asked for after a download wins over it.
    if removal_marker(dir).exists() {
        if let Err(e) = remove_at_with(dir, rename) {
            dioxus::logger::tracing::warn!(
                "Could not finish removing the downloaded Orbbec SDK: {e}"
            );
        }
        return;
    }
    let staging = staging_dir(dir);
    if staging.join(STAGED_OK).is_file() {
        if let Err(e) = swap_in(&staging, dir, rename) {
            dioxus::logger::tracing::warn!("Could not switch to the downloaded Orbbec SDK: {e}");
        }
    }
}

/// Completes what had to wait for a restart: a removal, or a downloaded
/// replacement. Call it at startup, before anything can load the library.
pub fn finish_pending() {
    if let Some(dir) = managed_dir() {
        finish_pending_at(&dir, &rename_fs);
    }
}

#[cfg(feature = "sdk-download")]
pub use download::{Installed, Phase, install};

#[cfg(feature = "sdk-download")]
mod download {
    use std::io::{Cursor, Read};
    use std::path::Path;

    use anyhow::{Context, Result, bail};
    use sha2::{Digest, Sha256};

    use super::{STAGED_OK, install_path, managed_dir, pin, rename_fs, staging_dir, swap_in};

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

    /// How an install ended.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Installed {
        /// The new copy is in place; the loader finds it from now on.
        Ready,
        /// The copy it replaces is loaded and cannot be moved, so the new one waits
        /// beside it and takes over the next time the app starts.
        AfterRestart,
    }

    /// Downloads, verifies and unpacks the pinned SDK into the app's data folder.
    /// The previous copy is replaced only once the new one is complete.
    pub async fn install(mut progress: impl FnMut(Phase)) -> Result<Installed> {
        let dest = managed_dir().context("this system has no app data folder")?;
        let pin = pin();

        let archive = fetch(&pin.windows_x64.url, pin.windows_x64.size, &mut progress).await?;
        progress(Phase::Verifying);
        verify(&archive, &pin.windows_x64.sha256).context("the Orbbec SDK download is corrupt")?;

        let eula = fetch(&pin.eula.url, 0, &mut |_| {}).await?;
        verify(&eula, &pin.eula.sha256).context("the Orbbec licence download is corrupt")?;

        progress(Phase::Installing);
        let staging = staging_dir(&dest);
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        let unpacked = extract(&archive, &pin.windows_x64.files, &staging)
            .and_then(|()| std::fs::write(staging.join("EULA.txt"), &eula).map_err(Into::into))
            .and_then(|()| std::fs::write(staging.join(STAGED_OK), b"").map_err(Into::into));
        if let Err(e) = unpacked {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
        match swap_in(&staging, &dest, &rename_fs) {
            Ok(()) => Ok(Installed::Ready),
            Err(e) => {
                // Nothing of the old copy was touched. The finished one stays where
                // it is and is switched in at the next start.
                dioxus::logger::tracing::info!(
                    "The Orbbec SDK in {} is in use; the new one takes over at the next start ({e})",
                    dest.display()
                );
                Ok(Installed::AfterRestart)
            }
        }
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
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind};

    use super::*;

    fn version(major: u32, minor: u32, patch: u32) -> SdkVersion {
        SdkVersion {
            major,
            minor,
            patch,
        }
    }

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
    fn the_pinned_version_is_one_the_loader_accepts() {
        let pinned = pinned_version();
        assert!(
            pinned.is_supported(),
            "the app must be tested with a version it accepts: {pinned}"
        );
        assert_eq!(pinned.to_string(), pin().version);
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

    // ---- advisory

    fn running(found: SdkVersion, path: Option<&str>) -> RuntimeStatus {
        RuntimeStatus::Available {
            version: found,
            library_path: path.map(PathBuf::from),
        }
    }

    #[test]
    fn the_pinned_version_needs_no_advisory() {
        let pinned = version(2, 10, 6);
        assert_eq!(advisory(&running(pinned, None), None, pinned), None);
    }

    #[test]
    fn an_older_copy_we_downloaded_can_be_replaced() {
        let managed = Path::new("/data/Odyssey/orbbec-sdk");
        let status = running(
            version(2, 9, 0),
            Some("/data/Odyssey/orbbec-sdk/OrbbecSDK.dll"),
        );
        assert_eq!(
            advisory(&status, Some(managed), version(2, 10, 6)),
            Some(Advisory::Older {
                found: version(2, 9, 0),
                pinned: version(2, 10, 6),
                replaceable: true,
            })
        );
    }

    #[test]
    fn an_older_copy_found_elsewhere_is_only_warned_about() {
        let managed = Path::new("/data/Odyssey/orbbec-sdk");
        for path in [Some("/opt/orbbec/OrbbecSDK.dll"), None] {
            let status = running(version(2, 8, 3), path);
            assert_eq!(
                advisory(&status, Some(managed), version(2, 10, 6)),
                Some(Advisory::Older {
                    found: version(2, 8, 3),
                    pinned: version(2, 10, 6),
                    replaceable: false,
                }),
                "{path:?}"
            );
        }
    }

    #[test]
    fn a_newer_copy_is_a_warning_wherever_it_came_from() {
        let managed = Path::new("/data/Odyssey/orbbec-sdk");
        for path in [Some("/data/Odyssey/orbbec-sdk/OrbbecSDK.dll"), None] {
            let status = running(version(2, 11, 0), path);
            assert_eq!(
                advisory(&status, Some(managed), version(2, 10, 6)),
                Some(Advisory::Newer {
                    found: version(2, 11, 0),
                    pinned: version(2, 10, 6),
                })
            );
        }
    }

    #[test]
    fn a_patch_release_counts_too() {
        let status = running(version(2, 10, 5), None);
        assert!(matches!(
            advisory(&status, None, version(2, 10, 6)),
            Some(Advisory::Older { .. })
        ));
    }

    #[test]
    fn an_sdk_that_is_not_running_has_no_advisory() {
        let status = RuntimeStatus::NotFound { tried: Vec::new() };
        assert_eq!(advisory(&status, None, version(2, 10, 6)), None);
    }

    // ---- replacing and removing

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("odyssey-sdk-swap-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A copy of the SDK with a library and an extension, as `contents` says.
    fn copy(dir: &Path, contents: &str) {
        std::fs::create_dir_all(dir.join("extensions")).unwrap();
        std::fs::write(dir.join("OrbbecSDK.dll"), contents).unwrap();
        std::fs::write(dir.join("extensions/ext.dll"), contents).unwrap();
    }

    fn read(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("OrbbecSDK.dll")).unwrap()
    }

    /// Windows refusing to move a directory with a mapped library in it.
    fn in_use(_: &Path, _: &Path) -> io::Result<()> {
        Err(Error::new(ErrorKind::PermissionDenied, "in use"))
    }

    #[test]
    fn a_finished_download_replaces_the_old_copy_whole() {
        let root = scratch("replace");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");
        let staging = staging_dir(&dest);
        copy(&staging, "new");

        swap_in(&staging, &dest, &rename_fs).unwrap();

        assert_eq!(read(&dest), "new");
        assert!(!staging.exists());
        assert!(
            !sibling(&dest, "old").exists(),
            "the old copy is cleaned up"
        );
        assert!(
            !dest.join(STAGED_OK).exists(),
            "the marker does not outlive the swap"
        );
    }

    #[test]
    fn a_copy_in_use_is_left_exactly_as_it_was() {
        // The library is mapped, so the move fails. Nothing may have been deleted by
        // then: a running SDK loads its extensions lazily.
        let root = scratch("in-use");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");
        let staging = staging_dir(&dest);
        copy(&staging, "new");

        assert!(swap_in(&staging, &dest, &in_use).is_err());

        assert_eq!(read(&dest), "old");
        assert_eq!(
            std::fs::read_to_string(dest.join("extensions/ext.dll")).unwrap(),
            "old"
        );
        assert_eq!(
            read(&staging),
            "new",
            "the download waits for the next start"
        );
    }

    #[test]
    fn a_failed_swap_puts_the_old_copy_back() {
        // Moving the old copy aside works but moving the new one in does not.
        let root = scratch("restore");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");
        let staging = staging_dir(&dest);
        copy(&staging, "new");
        let only_the_first_move = |from: &Path, to: &Path| {
            if from == staging {
                Err(Error::other("cannot move the new copy"))
            } else {
                std::fs::rename(from, to)
            }
        };

        assert!(swap_in(&staging, &dest, &only_the_first_move).is_err());

        assert_eq!(read(&dest), "old", "the user still has a working SDK");
    }

    #[test]
    fn the_next_start_switches_to_a_finished_download() {
        let root = scratch("next-start");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");
        let staging = staging_dir(&dest);
        copy(&staging, "new");
        std::fs::write(staging.join(STAGED_OK), b"").unwrap();

        finish_pending_at(&dest, &rename_fs);

        assert_eq!(read(&dest), "new");
        assert!(!staging.exists());
    }

    #[test]
    fn the_next_start_ignores_a_download_that_did_not_finish() {
        let root = scratch("unfinished");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");
        let staging = staging_dir(&dest);
        copy(&staging, "half"); // no STAGED_OK

        finish_pending_at(&dest, &rename_fs);

        assert_eq!(read(&dest), "old");
    }

    #[test]
    fn removing_a_copy_deletes_it_and_a_waiting_download() {
        let root = scratch("remove");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");
        copy(&staging_dir(&dest), "new");
        std::fs::write(removal_marker(&dest), b"").unwrap(); // left by an earlier attempt

        assert_eq!(remove_at_with(&dest, &rename_fs).unwrap(), Removal::Removed);

        assert!(!dest.exists());
        assert!(!staging_dir(&dest).exists());
        assert!(!removal_marker(&dest).exists());
    }

    #[test]
    fn removing_a_copy_that_is_already_gone_is_not_an_error() {
        let root = scratch("gone");
        assert_eq!(
            remove_at_with(&root.join(DIR_NAME), &rename_fs).unwrap(),
            Removal::Removed
        );
    }

    #[test]
    fn a_copy_in_use_is_marked_for_removal_and_not_touched() {
        let root = scratch("remove-in-use");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");

        assert_eq!(
            remove_at_with(&dest, &in_use).unwrap(),
            Removal::AfterRestart
        );

        assert!(
            removal_marker(&dest).exists(),
            "the next start finishes the job"
        );
        assert_eq!(read(&dest), "old");
        assert_eq!(
            std::fs::read_to_string(dest.join("extensions/ext.dll")).unwrap(),
            "old"
        );

        // The next start, with nothing holding it, completes the removal.
        finish_pending_at(&dest, &rename_fs);
        assert!(!dest.exists() && !removal_marker(&dest).exists());
    }

    #[test]
    fn a_removal_asked_for_after_a_download_wins_at_the_next_start() {
        let root = scratch("remove-wins");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");
        let staging = staging_dir(&dest);
        copy(&staging, "new");
        std::fs::write(staging.join(STAGED_OK), b"").unwrap();
        std::fs::write(removal_marker(&dest), b"").unwrap();

        finish_pending_at(&dest, &rename_fs);

        assert!(!dest.exists(), "the user asked for it to go");
        assert!(!staging.exists());
    }
}
