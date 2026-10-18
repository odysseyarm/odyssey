//! What Setup tells the user when the camera library is not usable.
//!
//! The wording lives here, apart from the views, so each situation's message
//! is plain data a test can read.

use odyssey_autocalib::orbbec::{RuntimeStatus, LIBRARY_NAME, SDK_DIR_ENV};

/// Where Orbbec publishes the SDK. The latest release is always a 2.x one the
/// backend accepts.
pub const SDK_DOWNLOAD_URL: &str = "https://github.com/orbbec/OrbbecSDK_v2/releases/latest";

/// How Setup presents an unusable camera library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    /// One or two plain sentences for everybody.
    pub detail: String,
    /// Technical lines, collapsed under "Details".
    pub details: Vec<String>,
    /// Whether to offer a button that opens [`SDK_DOWNLOAD_URL`].
    pub offer_download: bool,
}

/// The message for `status`, or `None` when cameras can be listed.
pub fn notice(status: &RuntimeStatus) -> Option<Notice> {
    match status {
        RuntimeStatus::Available { .. } => None,
        RuntimeStatus::NotFound { tried } => {
            let mut details = vec!["The camera library was looked for in this order:".to_string()];
            details.extend(tried.iter().map(|line| format!("• {line}")));
            details.push(format!(
                "To use an install in another location, set the environment variable {SDK_DIR_ENV} to the \
                 folder that contains {LIBRARY_NAME}, then restart the app."
            ));
            Some(Notice {
                title: "Camera support isn't installed".to_string(),
                detail: "Calibrating with a camera needs Orbbec's free camera software. Get it, install it, \
                         and this page notices by itself."
                    .to_string(),
                details,
                offer_download: true,
            })
        }
        RuntimeStatus::LoadFailed { library, error } => Some(Notice {
            title: "Camera support couldn't start".to_string(),
            detail: "The camera library was found but could not be loaded. Restarting the app or the \
                     computer sometimes clears this."
                .to_string(),
            details: vec![format!("Library: {library}"), format!("Error: {error}")],
            offer_download: false,
        }),
        RuntimeStatus::UnsupportedVersion { found, supported } => Some(Notice {
            title: "The camera software is the wrong version".to_string(),
            detail: format!("Orbbec SDK {found} is installed. This app works with Orbbec SDK {supported}."),
            details: vec![format!("Installed version: {found}"), format!("Works with: {supported}")],
            offer_download: true,
        }),
    }
}

#[cfg(test)]
mod tests {
    use odyssey_autocalib::orbbec::SdkVersion;

    use super::*;

    fn not_found() -> RuntimeStatus {
        RuntimeStatus::NotFound {
            tried: vec![
                format!("{SDK_DIR_ENV} is not set"),
                "libOrbbecSDK.so.2 through the system library search path".to_string(),
            ],
        }
    }

    #[test]
    fn a_usable_library_needs_no_notice() {
        let status = RuntimeStatus::Available {
            version: SdkVersion { major: 2, minor: 10, patch: 6 },
            library_path: None,
        };
        assert_eq!(notice(&status), None);
    }

    #[test]
    fn missing_library_offers_the_download_and_explains_the_override() {
        let n = notice(&not_found()).unwrap();
        assert_eq!(n.title, "Camera support isn't installed");
        assert!(n.offer_download);
        let details = n.details.join("\n");
        assert!(details.contains("libOrbbecSDK.so.2 through the system library search path"), "{details}");
        assert!(details.contains(&format!("{SDK_DIR_ENV} is not set")), "{details}");
        assert!(details.contains(&format!("set the environment variable {SDK_DIR_ENV}")), "{details}");
    }

    #[test]
    fn missing_library_lists_what_was_tried_in_order() {
        let details = notice(&not_found()).unwrap().details;
        let position = |needle: &str| details.iter().position(|l| l.contains(needle)).unwrap();
        assert!(position("is not set") < position("system library search path"));
    }

    #[test]
    fn broken_library_shows_the_loader_error_and_never_says_reinstall() {
        let status = RuntimeStatus::LoadFailed {
            library: "/opt/orbbec/libOrbbecSDK.so.2".to_string(),
            error: "libudev.so.1: cannot open shared object file".to_string(),
        };
        let n = notice(&status).unwrap();
        assert!(n.detail.starts_with("The camera library was found but could not be loaded"));
        assert!(!n.offer_download);
        let everything = format!("{} {} {}", n.title, n.detail, n.details.join(" ")).to_lowercase();
        assert!(!everything.contains("install"), "{everything}");
        let details = n.details.join("\n");
        assert!(details.contains("/opt/orbbec/libOrbbecSDK.so.2"), "{details}");
        assert!(details.contains("libudev.so.1: cannot open shared object file"), "{details}");
    }

    #[test]
    fn wrong_version_names_the_found_and_the_working_versions() {
        let status = RuntimeStatus::UnsupportedVersion {
            found: SdkVersion { major: 1, minor: 9, patch: 4 },
            supported: "2.8 or newer in the 2.x series (tested with 2.10.6)",
        };
        let n = notice(&status).unwrap();
        assert!(n.detail.contains("1.9.4"), "{}", n.detail);
        assert!(n.detail.contains("2.8 or newer"), "{}", n.detail);
        assert!(n.offer_download);
    }
}
