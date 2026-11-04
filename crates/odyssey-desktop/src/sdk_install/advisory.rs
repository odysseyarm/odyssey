use std::cmp::Ordering;
use std::path::Path;

use odyssey_autocalib::orbbec::{RuntimeStatus, SdkVersion};

use super::locations::{managed_dir, search_dir};
use super::pin_file::pinned_version;

/// What to tell the user about a working SDK that is not the version this build
/// is tested with. A library the loader accepts (2.8 or newer) can still differ
/// from the pin; it is only a warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Advisory {
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
pub(crate) fn advisory(
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
pub(crate) fn current_advisory() -> Option<Advisory> {
    let status = odyssey_autocalib::orbbec::runtime_status(search_dir().as_deref());
    advisory(&status, managed_dir().as_deref(), pinned_version())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn version(major: u32, minor: u32, patch: u32) -> SdkVersion {
        SdkVersion {
            major,
            minor,
            patch,
        }
    }

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
}
