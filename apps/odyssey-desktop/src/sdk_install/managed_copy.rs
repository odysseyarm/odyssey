use std::io;
use std::path::{Path, PathBuf};

use super::locations::{DIR_NAME, managed_dir};

/// How a removal ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Removal {
    Removed,
    /// The running copy is in use and stays intact; it is deleted the next time
    /// the app starts, by [`finish_pending`].
    AfterRestart,
}

/// Only a refused move of the running copy can be deferred. Other failures must
/// reach the caller rather than being presented as a successful installation.
pub(super) enum Swap {
    Ready,
    AfterRestart,
}

enum MoveAside {
    Missing,
    Moved(PathBuf),
    InUse(io::Error),
}

/// Renames a directory; a parameter so tests can stand in for a mapped library
/// that Windows will not let go of.
type Rename<'a> = &'a dyn Fn(&Path, &Path) -> io::Result<()>;

pub(super) fn rename_fs(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

fn sibling(dir: &Path, suffix: &str) -> PathBuf {
    dir.with_file_name(format!("{DIR_NAME}.{suffix}"))
}

/// Where a finished download waits when the copy it replaces is in use.
pub(super) fn staging_dir(dir: &Path) -> PathBuf {
    sibling(dir, "new")
}

/// Written into a staged copy once it is complete, so a download interrupted
/// halfway is never swapped in.
pub(super) const STAGED_OK: &str = ".staged";

fn removal_marker(dir: &Path) -> PathBuf {
    sibling(dir, "remove")
}

/// Include staged downloads and cleanup remnants so the UI's removal action
/// remains available even when no library has reached its final location.
pub(crate) fn has_managed_copy() -> bool {
    managed_dir().is_some_and(|dir| {
        dir.exists()
            || staging_dir(&dir).exists()
            || sibling(&dir, "old").exists()
            || removal_marker(&dir).exists()
    })
}

pub(super) fn remove_dir_if_present(dir: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn remove_file_if_present(file: &Path) -> io::Result<()> {
    match std::fs::remove_file(file) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Invalidate an earlier completed download before deleting any of its files.
/// If cleanup fails halfway, startup must not activate that incomplete copy.
#[cfg(any(feature = "sdk-download", test))]
pub(super) fn prepare_staging(dir: &Path) -> io::Result<PathBuf> {
    let staging = staging_dir(dir);
    match std::fs::symlink_metadata(&staging) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            remove_file_if_present(&staging.join(STAGED_OK))?;
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    remove_dir_if_present(&staging)?;
    if let Some(parent) = staging.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Unlike create_dir_all, this refuses to reuse a directory or symlink that
    // appeared after cleanup.
    std::fs::create_dir(&staging)?;
    Ok(staging)
}

/// Moves the entire copy or leaves it untouched. A failure cleaning an older
/// backup is not evidence that the running library is locked.
fn move_aside(dir: &Path, rename: Rename<'_>) -> io::Result<MoveAside> {
    if !dir.try_exists()? {
        return Ok(MoveAside::Missing);
    }
    let old = sibling(dir, "old");
    remove_dir_if_present(&old)?;
    match rename(dir, &old) {
        Ok(()) => Ok(MoveAside::Moved(old)),
        // Windows sharing/lock violations are not mapped to PermissionDenied
        // by std, but also mean the mapped copy must wait for a restart.
        Err(e)
            if e.kind() == io::ErrorKind::PermissionDenied
                || (cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33))) =>
        {
            Ok(MoveAside::InUse(e))
        }
        Err(e) => Err(e),
    }
}

/// Puts the finished copy in place, restoring the old one on failure. If even
/// rollback fails, retain the backup and report both failures and its location.
pub(super) fn swap_in(staging: &Path, dest: &Path, rename: Rename<'_>) -> io::Result<Swap> {
    // Startup acts on a pending removal before any download can run, so reaching
    // this point means the download supersedes that earlier removal request.
    remove_file_if_present(&removal_marker(dest))?;
    let old = match move_aside(dest, rename)? {
        MoveAside::Missing => None,
        MoveAside::Moved(old) => Some(old),
        MoveAside::InUse(e) => {
            dioxus::logger::tracing::info!(
                "Could not move the Orbbec SDK in {}; the update waits for a restart: {e}",
                dest.display()
            );
            return Ok(Swap::AfterRestart);
        }
    };
    if let Err(e) = rename(staging, dest) {
        if let Some(old) = &old
            && let Err(rollback) = rename(old, dest)
        {
            return Err(io::Error::other(format!(
                "could not install the SDK: {e}; could not restore the previous copy from {}: {rollback}",
                old.display()
            )));
        }
        return Err(e);
    }
    if let Err(e) = remove_file_if_present(&dest.join(STAGED_OK)) {
        dioxus::logger::tracing::warn!("Could not clean the installed SDK's staging marker: {e}");
    }
    if let Some(old) = old
        && let Err(e) = remove_dir_if_present(&old)
    {
        dioxus::logger::tracing::warn!(
            "Could not clean the previous Orbbec SDK in {}: {e}",
            old.display()
        );
    }
    Ok(Swap::Ready)
}

fn remove_at_with(dir: &Path, rename: Rename<'_>) -> io::Result<Removal> {
    let marker = removal_marker(dir);
    let staging = staging_dir(dir);
    let old = sibling(dir, "old");
    if !dir.try_exists()? && !staging.try_exists()? && !old.try_exists()? {
        remove_file_if_present(&marker)?;
        return Ok(Removal::Removed);
    }
    // Persist the intent before cleanup. In particular, a failed cleanup must
    // never leave a finished staged download free to reinstall at the next start.
    match std::fs::File::create_new(&marker) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && marker.is_file() => {}
        Err(e) => return Err(e),
    }
    remove_dir_if_present(&staging)?;
    match move_aside(dir, rename)? {
        MoveAside::Missing => remove_dir_if_present(&old)?,
        MoveAside::Moved(old) => remove_dir_if_present(&old)?,
        MoveAside::InUse(e) => {
            dioxus::logger::tracing::info!(
                "Could not move the Orbbec SDK in {}; removal waits for a restart: {e}",
                dir.display()
            );
            return Ok(Removal::AfterRestart);
        }
    }
    remove_file_if_present(&marker)?;
    Ok(Removal::Removed)
}

/// Deletes the downloaded copy, or arranges for it to be deleted at the next
/// start when it is in use.
pub(crate) fn remove_managed_copy() -> io::Result<Removal> {
    match managed_dir() {
        Some(dir) => remove_at_with(&dir, &rename_fs),
        None => Ok(Removal::Removed),
    }
}

/// Whether a removal is waiting for the next start.
pub(crate) fn removal_pending() -> bool {
    managed_dir().is_some_and(|dir| removal_marker(&dir).exists())
}

/// Whether a downloaded replacement is waiting for the next start.
pub(crate) fn update_pending() -> bool {
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
        match swap_in(&staging, dir, rename) {
            Ok(Swap::Ready) => {}
            Ok(Swap::AfterRestart) => dioxus::logger::tracing::warn!(
                "The downloaded Orbbec SDK is still in use; its update remains pending"
            ),
            Err(e) => {
                dioxus::logger::tracing::warn!(
                    "Could not switch to the downloaded Orbbec SDK: {e}"
                );
            }
        }
    }
}

/// Completes what had to wait for a restart: a removal, or a downloaded
/// replacement. Call it at startup, before anything can load the library.
pub(crate) fn finish_pending() {
    if let Some(dir) = managed_dir() {
        finish_pending_at(&dir, &rename_fs);
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind};

    use super::*;

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

        assert!(matches!(
            swap_in(&staging, &dest, &in_use).unwrap(),
            Swap::AfterRestart
        ));

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
                Err(Error::new(
                    ErrorKind::PermissionDenied,
                    "cannot move the new copy",
                ))
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
    #[test]
    fn a_later_download_supersedes_a_deferred_removal() {
        let root = scratch("download-after-remove");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");
        assert_eq!(
            remove_at_with(&dest, &in_use).unwrap(),
            Removal::AfterRestart
        );
        let staging = prepare_staging(&dest).unwrap();
        copy(&staging, "new");
        std::fs::write(staging.join(STAGED_OK), b"").unwrap();

        assert!(matches!(
            swap_in(&staging, &dest, &in_use).unwrap(),
            Swap::AfterRestart
        ));
        finish_pending_at(&dest, &rename_fs);

        assert_eq!(read(&dest), "new");
        assert!(!removal_marker(&dest).exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_sharing_violation_defers_without_modifying_the_running_copy() {
        let root = scratch("sharing-violation");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");
        let staging = staging_dir(&dest);
        copy(&staging, "new");
        let sharing_violation = |_: &Path, _: &Path| Err(Error::from_raw_os_error(32));

        assert!(matches!(
            swap_in(&staging, &dest, &sharing_violation).unwrap(),
            Swap::AfterRestart
        ));
        assert_eq!(read(&dest), "old");
        assert_eq!(read(&staging), "new");
    }

    #[test]
    fn a_non_lock_move_failure_is_reported_without_touching_the_old_copy() {
        let root = scratch("fatal-move");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");
        let staging = staging_dir(&dest);
        copy(&staging, "new");
        let fail = |_: &Path, _: &Path| Err(Error::other("filesystem failure"));

        let error = swap_in(&staging, &dest, &fail).err().unwrap();

        assert_eq!(error.to_string(), "filesystem failure");
        assert_eq!(read(&dest), "old");
        assert_eq!(read(&staging), "new");
    }

    #[test]
    fn a_failed_rollback_reports_both_errors_and_retains_the_backup() {
        let root = scratch("failed-rollback");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");
        let staging = staging_dir(&dest);
        copy(&staging, "new");
        let fail_after_move = |from: &Path, to: &Path| {
            if from == dest {
                std::fs::rename(from, to)
            } else if from == staging {
                Err(Error::other("install failed"))
            } else {
                Err(Error::other("rollback failed"))
            }
        };

        let error = swap_in(&staging, &dest, &fail_after_move).err().unwrap();
        let message = error.to_string();
        assert!(message.contains("install failed"), "{message}");
        assert!(message.contains("rollback failed"), "{message}");
        assert_eq!(read(&sibling(&dest, "old")), "old");
        assert_eq!(read(&staging), "new");
    }

    #[test]
    fn failed_staging_cleanup_does_not_report_removal_or_reinstall_the_sdk() {
        let root = scratch("failed-remove-cleanup");
        let dest = root.join(DIR_NAME);
        copy(&dest, "old");
        // A damaged staging path is not a directory and cannot be cleaned with
        // remove_dir_all. It must not be ignored while claiming success.
        std::fs::write(staging_dir(&dest), b"damaged").unwrap();

        assert!(remove_at_with(&dest, &rename_fs).is_err());
        assert!(removal_marker(&dest).exists());
        finish_pending_at(&dest, &rename_fs);
        assert_eq!(read(&dest), "old");

        std::fs::remove_file(staging_dir(&dest)).unwrap();
        finish_pending_at(&dest, &rename_fs);
        assert!(!dest.exists());
        assert!(!removal_marker(&dest).exists());
    }
}
