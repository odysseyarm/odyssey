//! What the administrator who deployed Odyssey has switched off.
//!
//! One switch matters today: **offline mode**. With it on, no feature may open a
//! connection to the internet: firmware comes from a folder on disk (see
//! `firmware::pack`) instead of GitHub, and the camera SDK is never downloaded. Code
//! that is about to use the network asks [`offline`] first, so a new feature has one
//! obvious thing to check.
//!
//! The policy is read from places only an administrator can write, and **any of them
//! can turn offline mode on; none can turn it off.** A stale file cannot undo a
//! registry setting or the other way round, and the environment can only tighten it.
//! A policy file that cannot be read or understood turns offline mode on as well
//! ([`Policy::issues`] says why): an administrator who wrote one wanted it enforced.
//!
//! Sources, in the order they are consulted for the firmware folder:
//!
//! 1. the environment: `ODYSSEY_OFFLINE=1` and `ODYSSEY_FIRMWARE_PACK_DIR`, for
//!    testing and for wrappers (it cannot switch offline mode off);
//! 2. on Windows, the registry (`HKLM\SOFTWARE\Policies\OdysseyArm\Odyssey`, then
//!    `HKLM\SOFTWARE\OdysseyArm\Odyssey`): `OfflineMode` (DWORD) and `FirmwarePackDir`
//!    (string), which is what Group Policy and Intune deploy;
//! 3. the machine-wide policy file, on every platform ([`policy_file_path`]). It is
//!    JSON5, so it may carry comments: `{ "offline": true, "firmware_pack_dir": "D:\\pack" }`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::Deserialize;

/// The merged policy.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Policy {
    /// No feature may connect to the internet.
    pub offline: bool,
    /// Where the offline firmware pack is, if the administrator said.
    pub firmware_pack_dir: Option<PathBuf>,
    /// Sources that could not be read, for whoever is looking at the screen.
    pub issues: Vec<String>,
}

/// What one source says.
#[derive(Debug, Default, PartialEq, Eq)]
struct Source {
    offline: bool,
    firmware_pack_dir: Option<PathBuf>,
    issue: Option<String>,
}

/// The shape of the policy file. Unknown fields are ignored so a newer file still
/// works with an older Odyssey.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FileContents {
    offline: bool,
    firmware_pack_dir: Option<PathBuf>,
}

/// Reads one policy file's text. Text that is not valid policy turns offline mode
/// on, because whoever wrote the file meant it to be enforced.
fn source_from_text(origin: &str, text: &str) -> Source {
    match json5::from_str::<FileContents>(text) {
        Ok(contents) => Source {
            offline: contents.offline,
            firmware_pack_dir: contents
                .firmware_pack_dir
                .filter(|p| !p.as_os_str().is_empty()),
            issue: None,
        },
        Err(e) => Source {
            offline: true,
            firmware_pack_dir: None,
            issue: Some(format!(
                "{origin} could not be understood ({e}); offline mode is on until it is fixed"
            )),
        },
    }
}

fn source_from_file(path: &Path) -> Option<Source> {
    let origin = path.display().to_string();
    match std::fs::read_to_string(path) {
        Ok(text) => Some(source_from_text(&origin, &text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(Source {
            offline: true,
            firmware_pack_dir: None,
            issue: Some(format!(
                "{origin} could not be read ({e}); offline mode is on until it is fixed"
            )),
        }),
    }
}

/// Combines sources, highest priority first. Offline if any source says so; the
/// firmware folder is the first one named.
fn merge(sources: impl IntoIterator<Item = Source>) -> Policy {
    let mut policy = Policy::default();
    for source in sources {
        policy.offline |= source.offline;
        if policy.firmware_pack_dir.is_none() {
            policy.firmware_pack_dir = source.firmware_pack_dir;
        }
        policy.issues.extend(source.issue);
    }
    policy
}

fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// The environment can only ever tighten the policy: it has no way to say "online".
fn environment_source(get: impl Fn(&str) -> Option<String>) -> Source {
    Source {
        offline: get("ODYSSEY_OFFLINE").is_some_and(|v| truthy(&v)),
        firmware_pack_dir: get("ODYSSEY_FIRMWARE_PACK_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
        issue: None,
    }
}

/// Where the machine-wide policy file is: a place a standard user cannot write.
pub fn policy_file_path() -> PathBuf {
    machine_dir().join("policy.json")
}

/// The folder Odyssey looks for the offline firmware pack in when policy does not
/// name one.
pub fn default_firmware_pack_dir() -> PathBuf {
    if cfg!(any(windows, target_os = "macos")) {
        machine_dir().join("firmware-pack")
    } else {
        PathBuf::from("/var/lib/odyssey/firmware-pack")
    }
}

fn machine_dir() -> PathBuf {
    if cfg!(windows) {
        let base = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        base.join("OdysseyArm").join("Odyssey")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/OdysseyArm/Odyssey")
    } else {
        PathBuf::from("/etc/odyssey")
    }
}

/// What Group Policy, Intune or the installer wrote under HKLM.
#[cfg(windows)]
fn registry_sources() -> Vec<Source> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    [
        r"SOFTWARE\Policies\OdysseyArm\Odyssey",
        r"SOFTWARE\OdysseyArm\Odyssey",
    ]
    .into_iter()
    .filter_map(|path| {
        let key = hklm
            .open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY)
            .ok()?;
        Some(Source {
            offline: key.get_value::<u32, _>("OfflineMode").is_ok_and(|v| v != 0),
            firmware_pack_dir: key
                .get_value::<String, _>("FirmwarePackDir")
                .ok()
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            issue: None,
        })
    })
    .collect()
}

#[cfg(not(windows))]
fn registry_sources() -> Vec<Source> {
    Vec::new()
}

fn load() -> Policy {
    let env = |name: &str| std::env::var(name).ok();
    let mut sources = vec![environment_source(env)];
    sources.extend(registry_sources());
    // A second file named in the environment adds to the machine-wide one.
    if let Some(extra) = env("ODYSSEY_POLICY_FILE").filter(|v| !v.is_empty()) {
        sources.extend(source_from_file(Path::new(&extra)));
    }
    sources.extend(source_from_file(&policy_file_path()));

    let policy = merge(sources);
    for issue in &policy.issues {
        tracing::error!("Odyssey policy: {issue}");
    }
    if policy.offline {
        tracing::info!("Offline mode is on: nothing will connect to the internet");
    }
    policy
}

/// The policy, read once at first use: changing it takes a restart.
pub fn current() -> &'static Policy {
    static POLICY: LazyLock<Policy> = LazyLock::new(load);
    &POLICY
}

/// Whether offline mode is on. Check this before any feature touches the network.
pub fn offline() -> bool {
    current().offline
}

/// The folder the offline firmware pack is read from.
pub fn firmware_pack_dir() -> PathBuf {
    current()
        .firmware_pack_dir
        .clone()
        .unwrap_or_else(default_firmware_pack_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    fn file(text: &str) -> Source {
        source_from_text("policy.json", text)
    }

    #[test]
    fn with_no_policy_the_app_is_online() {
        assert_eq!(merge([]), Policy::default());
        assert!(!merge([file("{}")]).offline);
    }

    #[test]
    fn a_policy_file_can_turn_offline_mode_on_and_name_the_folder() {
        let policy = merge([file(
            r#"{ "offline": true, "firmware_pack_dir": "D:\\pack" }"#,
        )]);
        assert!(policy.offline);
        assert_eq!(policy.firmware_pack_dir, Some(PathBuf::from(r"D:\pack")));
        assert!(policy.issues.is_empty());
    }

    #[test]
    fn the_file_may_carry_comments_and_fields_this_version_does_not_know() {
        let policy = merge([file(
            "// set by IT\n{ offline: true, /* later */ future_setting: 7, }\n",
        )]);
        assert!(policy.offline);
        assert!(policy.issues.is_empty());
    }

    #[test]
    fn any_source_can_turn_offline_mode_on_and_none_can_turn_it_off() {
        let on = || file(r#"{"offline": true}"#);
        let off = || file(r#"{"offline": false}"#);
        assert!(merge([off(), on()]).offline);
        assert!(merge([on(), off()]).offline);
        assert!(!merge([off(), off()]).offline);
    }

    #[test]
    fn the_environment_can_tighten_but_never_loosen() {
        let on = environment_source(env_of(&[("ODYSSEY_OFFLINE", "1")]));
        assert!(on.offline);
        for value in ["0", "false", "no", "", "off", "maybe"] {
            let source = environment_source(move |name| {
                (name == "ODYSSEY_OFFLINE").then(|| value.to_string())
            });
            assert!(!source.offline, "{value:?} must not switch anything on");
        }
        // And it cannot undo a file that says offline.
        let policy = merge([
            environment_source(env_of(&[("ODYSSEY_OFFLINE", "0")])),
            file(r#"{"offline": true}"#),
        ]);
        assert!(policy.offline);
    }

    #[test]
    fn the_first_source_to_name_a_firmware_folder_wins() {
        let policy = merge([
            environment_source(env_of(&[("ODYSSEY_FIRMWARE_PACK_DIR", "/from/env")])),
            file(r#"{"firmware_pack_dir": "/from/file"}"#),
        ]);
        assert_eq!(policy.firmware_pack_dir, Some(PathBuf::from("/from/env")));
        let only_file = merge([file(r#"{"firmware_pack_dir": "/from/file"}"#)]);
        assert_eq!(
            only_file.firmware_pack_dir,
            Some(PathBuf::from("/from/file"))
        );
    }

    #[test]
    fn a_policy_file_that_is_broken_fails_closed_and_says_why() {
        for text in [
            "{ this is not json",
            r#"{"offline": "false"}"#,
            r#"{"offline": 1}"#,
        ] {
            let policy = merge([file(text)]);
            assert!(
                policy.offline,
                "{text}: whoever wrote it wanted it enforced"
            );
            assert_eq!(policy.issues.len(), 1, "{text}");
            assert!(
                policy.issues[0].contains("policy.json"),
                "{:?}",
                policy.issues
            );
        }
    }

    #[test]
    fn a_missing_file_is_just_no_policy_and_an_unreadable_one_fails_closed() {
        let dir = std::env::temp_dir().join(format!("odyssey-policy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(source_from_file(&dir.join("nope.json")), None);

        // A directory where the file should be cannot be read as one.
        let unreadable = source_from_file(&dir).unwrap();
        assert!(unreadable.offline);
        assert!(unreadable.issue.is_some());

        let good = dir.join("policy.json");
        std::fs::write(&good, r#"{"offline": true}"#).unwrap();
        assert!(source_from_file(&good).unwrap().offline);
    }

    #[test]
    fn an_empty_folder_name_means_not_named() {
        let policy = merge([file(r#"{"firmware_pack_dir": ""}"#)]);
        assert_eq!(policy.firmware_pack_dir, None);
    }
}
