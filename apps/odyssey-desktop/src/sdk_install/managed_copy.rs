use std::io;
use std::path::{Path, PathBuf};

use super::locations::{DIR_NAME, managed_dir};

/// How a removal ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Removal {
    Removed,
    /// The copy is in use, so nothing was touched; it is deleted the next time the
    /// app starts, by [`finish_pending`].
    AfterRestart,
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
pub(super) fn swap_in(staging: &Path, dest: &Path, rename: Rename<'_>) -> io::Result<()> {
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
    if staging.join(STAGED_OK).is_file()
        && let Err(e) = swap_in(&staging, dir, rename)
    {
        dioxus::logger::tracing::warn!("Could not switch to the downloaded Orbbec SDK: {e}");
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
