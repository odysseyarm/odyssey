use std::io::{Cursor, Read as _};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use sha2::{Digest as _, Sha256};

use super::locations::managed_dir;
use super::managed_copy::{STAGED_OK, rename_fs, staging_dir, swap_in};
use super::pin_file::{install_path, pin};

/// Largest file taken out of the archive. The real ones are a few MB; this
/// only stops a corrupt or hostile archive from filling the disk.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// What the download is doing, for a progress display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Downloading { received: u64, total: u64 },
    Verifying,
    Installing,
}

/// How an install ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Installed {
    /// The new copy is in place; the loader finds it from now on.
    Ready,
    /// The copy it replaces is loaded and cannot be moved, so the new one waits
    /// beside it and takes over the next time the app starts.
    AfterRestart,
}

/// Downloads, verifies and unpacks the pinned SDK into the app's data folder.
/// The previous copy is replaced only once the new one is complete.
pub(crate) async fn install(mut progress: impl FnMut(Phase)) -> Result<Installed> {
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

async fn fetch(url: &str, expected_size: u64, progress: &mut impl FnMut(Phase)) -> Result<Vec<u8>> {
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
fn verify(bytes: &[u8], expected: &str) -> Result<()> {
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
fn extract(archive: &[u8], files: &[String], dest: &Path) -> Result<()> {
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
    use std::io::Write as _;

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
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    fn tempdir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("odyssey-sdk-test-{}-{name}", std::process::id()));
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
        let err = extract(&zip, &names(&["bin/OrbbecSDK.dll", "bin/gone.dll"]), &dest).unwrap_err();
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
