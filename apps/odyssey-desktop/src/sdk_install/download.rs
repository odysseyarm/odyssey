use std::io::{Cursor, Read as _};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use sha2::{Digest as _, Sha256};

use super::locations::managed_dir;
use super::managed_copy::{
    STAGED_OK, Swap, prepare_staging, remove_dir_if_present, rename_fs, swap_in,
};
use super::pin_file::{install_path, pin};

/// Largest file taken out of the archive. The real ones are a few MB; this
/// only stops a corrupt or hostile archive from filling the disk.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EULA_BYTES: u64 = 1024 * 1024;

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
    /// The replacement is in place and the loader picks it up on its next search.
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

    let archive = fetch(
        &pin.windows_x64.url,
        pin.windows_x64.size,
        pin.windows_x64.size,
        &mut progress,
    )
    .await?;
    progress(Phase::Verifying);
    verify(&archive, &pin.windows_x64.sha256).context("the Orbbec SDK download is corrupt")?;

    let eula = fetch(&pin.eula.url, 0, MAX_EULA_BYTES, &mut |_| {}).await?;
    verify(&eula, &pin.eula.sha256).context("the Orbbec licence download is corrupt")?;

    progress(Phase::Installing);
    let staging = prepare_staging(&dest).context("could not prepare the SDK download folder")?;
    let unpacked = extract(&archive, &pin.windows_x64.files, &staging)
        .and_then(|()| std::fs::write(staging.join("EULA.txt"), &eula).map_err(Into::into))
        .and_then(|()| std::fs::write(staging.join(STAGED_OK), b"").map_err(Into::into));
    if let Err(e) = unpacked {
        if let Err(cleanup) = remove_dir_if_present(&staging) {
            dioxus::logger::tracing::warn!(
                "Could not clean the unfinished SDK download: {cleanup}"
            );
        }
        return Err(e);
    }
    match swap_in(&staging, &dest, &rename_fs)? {
        Swap::Ready => Ok(Installed::Ready),
        Swap::AfterRestart => Ok(Installed::AfterRestart),
    }
}

async fn fetch(
    url: &str,
    expected_size: u64,
    max_bytes: u64,
    progress: &mut impl FnMut(Phase),
) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("odyssey-desktop/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?
        .error_for_status()?;
    read_response(response, expected_size, max_bytes, progress).await
}

async fn read_response(
    mut response: reqwest::Response,
    expected_size: u64,
    max_bytes: u64,
    progress: &mut impl FnMut(Phase),
) -> Result<Vec<u8>> {
    let total = response.content_length().unwrap_or(expected_size);
    if total > max_bytes {
        bail!("the download is larger than the allowed {max_bytes} bytes");
    }
    let mut bytes = Vec::with_capacity(usize::try_from(total)?);
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() as u64 > max_bytes.saturating_sub(bytes.len() as u64) {
            bail!("the download is larger than the allowed {max_bytes} bytes");
        }
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

    // The list is pinned by this build, but reject unsafe destinations here as
    // well: a future pin edit must not permit traversal or Windows ADS paths.
    for file in files {
        let relative = install_path(file);
        if relative.contains(['\\', ':'])
            || relative
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            bail!("unsafe SDK installation path: {file}");
        }
    }

    for file in files {
        let entry = zip.by_index(index[file.as_str()])?;
        if entry.size() > MAX_FILE_BYTES {
            bail!("{file} is larger than expected");
        }
        let target = dest.join(install_path(file));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Refuse to overwrite an existing file or follow a pre-existing symlink.
        let mut output = std::fs::File::create_new(&target)
            .with_context(|| format!("could not create {}", target.display()))?;
        let copied = std::io::copy(&mut entry.take(MAX_FILE_BYTES + 1), &mut output)
            .with_context(|| format!("could not unpack {file}"))?;
        if copied > MAX_FILE_BYTES {
            bail!("{file} is larger than expected");
        }
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
    async fn response(raw: &'static [u8]) -> reqwest::Response {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                std::io::Read::read_exact(&mut stream, &mut byte).unwrap();
                request.push(byte[0]);
            }
            stream.write_all(raw).unwrap();
        });
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}/sdk"))
            .send()
            .await
            .unwrap();
        server.join().unwrap();
        response
    }

    #[tokio::test(flavor = "current_thread")]
    async fn an_oversized_content_length_is_rejected_before_reading_the_body() {
        let response =
            response(b"HTTP/1.1 200 OK\r\nContent-Length: 4294967296\r\nConnection: close\r\n\r\n")
                .await;
        let error = read_response(response, 3, 3, &mut |_| {})
            .await
            .unwrap_err();
        assert!(error.to_string().contains("larger than"), "{error}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_chunked_download_cannot_exceed_the_limit() {
        let response = response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\nabcd\r\n0\r\n\r\n",
        ).await;
        let error = read_response(response, 0, 3, &mut |_| {})
            .await
            .unwrap_err();
        assert!(error.to_string().contains("larger than"), "{error}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_download_at_the_limit_is_accepted_intact() {
        let response =
            response(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc").await;
        assert_eq!(
            read_response(response, 3, 3, &mut |_| {}).await.unwrap(),
            b"abc"
        );
    }

    #[test]
    fn unsafe_listed_paths_are_rejected_before_any_file_is_created() {
        let dest = tempdir("unsafe-listed");
        for path in [
            "../escape.dll",
            "/absolute.dll",
            "bin/../escape.dll",
            "bin/a.dll:stream",
        ] {
            let zip = archive(&[("bin/OrbbecSDK.dll", b"dll"), (path, b"unsafe")]);
            assert!(extract(&zip, &names(&["bin/OrbbecSDK.dll", path]), &dest).is_err());
            assert!(!dest.join("OrbbecSDK.dll").exists());
        }
    }

    #[test]
    fn an_existing_file_is_never_overwritten() {
        let dest = tempdir("existing-target");
        std::fs::write(dest.join("OrbbecSDK.dll"), b"original").unwrap();
        let zip = archive(&[("bin/OrbbecSDK.dll", b"replacement")]);

        assert!(extract(&zip, &names(&["bin/OrbbecSDK.dll"]), &dest).is_err());
        assert_eq!(
            std::fs::read(dest.join("OrbbecSDK.dll")).unwrap(),
            b"original"
        );
    }
}
