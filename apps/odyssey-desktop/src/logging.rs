//! The app's log: the terminal, and a file in the per-user log folder that a user can
//! send when something goes wrong. The same on every platform.
//!
//! `latest.log` is this run's log. The run before it is kept as `previous.log`, since
//! a problem is often looked at after the app was restarted. The folder and files are
//! owner-only on Unix: a log can name devices and screens.

use std::fs::{self, File};
use std::io;
use std::path::Path;

use dioxus::logger::tracing;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

use crate::paths;

const LATEST: &str = "latest.log";
const PREVIOUS: &str = "previous.log";

/// Starts logging to the terminal and, when its folder can be made, to `latest.log`.
/// A missing or read-only log folder must not stop the app, so it only costs the file.
pub fn init() {
    let file = paths::log_dir().and_then(|dir| match open_log_file(&dir) {
        Ok(file) => Some(file),
        Err(e) => {
            eprintln!("odyssey-desktop: could not open a log file in {dir:?}: {e}");
            None
        }
    });
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with(fmt::layer().compact())
        .with(file.map(|file| fmt::layer().with_ansi(false).with_writer(file)))
        .init();
}

/// Makes `dir`, moves the last `latest.log` to `previous.log`, and opens a fresh
/// `latest.log`.
fn open_log_file(dir: &Path) -> io::Result<File> {
    let mut folder = fs::DirBuilder::new();
    folder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        folder.mode(0o700);
    }
    folder.create(dir)?;

    let latest = dir.join(LATEST);
    if latest.exists() {
        // A rename over an existing file replaces it on every platform we ship.
        let _ = fs::rename(&latest, dir.join(PREVIOUS));
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(latest)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("odyssey-logging-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_last_run_is_kept_beside_the_current_one() {
        let dir = scratch("rotate");
        writeln!(open_log_file(&dir).unwrap(), "first run").unwrap();
        writeln!(open_log_file(&dir).unwrap(), "second run").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(LATEST)).unwrap(),
            "second run\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join(PREVIOUS)).unwrap(),
            "first run\n"
        );
        // Only two runs are kept: a third replaces the oldest.
        writeln!(open_log_file(&dir).unwrap(), "third run").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(PREVIOUS)).unwrap(),
            "second run\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_folder_and_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("private");
        drop(open_log_file(&dir).unwrap());
        drop(open_log_file(&dir).unwrap());
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(LATEST)), 0o600);
        assert_eq!(mode(&dir.join(PREVIOUS)), 0o600);
        fs::remove_dir_all(dir).unwrap();
    }
}
