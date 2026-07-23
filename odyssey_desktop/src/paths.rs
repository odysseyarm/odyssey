use std::path::PathBuf;

/// Velopack pack id — must match --packId in the Justfile.
pub const PACK_ID: &str = "Odyssey.Desktop";

/// Root of the velopack install (%LOCALAPPDATA%\Odyssey.Desktop). Velopack
/// only replaces its current\ subdir on update and deletes the whole root on
/// uninstall, so desktop-owned files here (settings, logs) survive updates
/// and are cleaned up automatically. Hub-domain data (device offsets,
/// calibrations, …) lives under odyssey_hub_common's app_dirs2 identity
/// instead and deliberately survives uninstall.
pub fn app_root() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join(PACK_ID))
}

pub fn log_dir() -> Option<PathBuf> {
    app_root().map(|dir| dir.join("logs"))
}

pub fn settings_dir() -> Option<PathBuf> {
    app_root()
}
