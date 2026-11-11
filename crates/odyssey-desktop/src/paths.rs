use std::path::PathBuf;

/// Name of the app's directory under %LOCALAPPDATA%.
const APP_DIR_NAME: &str = "Odyssey.Desktop";

/// Root for desktop-owned files. Hub-domain data such as device offsets and
/// calibrations lives under `odyssey_hub_common`'s `app_dirs2` identity instead, so
/// uninstalling the app leaves it in place.
fn app_root() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join(APP_DIR_NAME))
}

/// Where the app's log files go: `%LOCALAPPDATA%\Odyssey.Desktop\logs` when
/// `LOCALAPPDATA` is set, as on Windows, otherwise `$XDG_STATE_HOME/odyssey-desktop/logs`
/// with the XDG default of `~/.local/state` when that variable is unset or empty.
pub(crate) fn log_dir() -> Option<PathBuf> {
    app_root()
        .map(|dir| dir.join("logs"))
        .or_else(state_log_dir)
}

fn state_log_dir() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })?;
    Some(state.join("odyssey-desktop").join("logs"))
}

/// Where a downloaded Orbbec SDK is kept (see `sdk_install`).
pub(crate) fn orbbec_sdk_dir() -> Option<PathBuf> {
    app_root().map(|dir| dir.join("orbbec-sdk"))
}
