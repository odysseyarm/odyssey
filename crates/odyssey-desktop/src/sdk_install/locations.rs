use std::path::{Path, PathBuf};

use odyssey_autocalib::orbbec::LIBRARY_NAME;

use crate::paths;

/// Directory name of an SDK copy, under the app (full installer) or under the
/// app's data folder (download).
pub(super) const DIR_NAME: &str = "orbbec-sdk";

/// The directory the full installer puts the SDK in.
fn bundled_dir() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join(DIR_NAME))
}

/// The directory a download is kept in.
pub(super) fn managed_dir() -> Option<PathBuf> {
    paths::orbbec_sdk_dir()
}

fn holds_library(dir: &Path) -> bool {
    dir.join(LIBRARY_NAME).is_file()
}

/// The directory to hand the loader as the application's own copy, if there is
/// one. The installer's copy wins over a download.
pub(crate) fn search_dir() -> Option<PathBuf> {
    [bundled_dir(), managed_dir()]
        .into_iter()
        .flatten()
        .find(|dir| holds_library(dir))
}
