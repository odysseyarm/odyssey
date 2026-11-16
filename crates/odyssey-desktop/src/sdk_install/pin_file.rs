use std::sync::LazyLock;

use odyssey_autocalib::orbbec::SdkVersion;
use serde::Deserialize;

const PIN_JSON: &str = include_str!("../../orbbec-sdk.json");

// Only the online build downloads, so only it reads the URLs, hashes and files.
#[derive(Debug, Deserialize)]
pub(crate) struct Pin {
    pub version: String,
    #[cfg_attr(
        not(any(feature = "sdk-download", test)),
        expect(dead_code, reason = "only the online build reads the download details")
    )]
    pub eula: Remote,
    #[cfg_attr(
        not(any(feature = "sdk-download", test)),
        expect(dead_code, reason = "only the online build reads the download details")
    )]
    pub windows_x64: Archive,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(
    not(any(feature = "sdk-download", test)),
    expect(dead_code, reason = "only the online build reads the download details")
)]
pub(crate) struct Remote {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(
    not(feature = "sdk-download"),
    expect(
        dead_code,
        reason = "only the online build downloads, which is the one reader of the archive size"
    )
)]
pub(crate) struct Archive {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// Paths inside the archive, as `/`-separated names.
    pub files: Vec<String>,
}

pub(crate) fn pin() -> &'static Pin {
    static PIN: LazyLock<Pin> = LazyLock::new(|| {
        serde_json::from_str(PIN_JSON).expect("orbbec-sdk.json is valid; a test parses it")
    });
    &PIN
}

/// The pinned version as the loader reports versions.
pub(super) fn pinned_version() -> SdkVersion {
    let mut parts = pin().version.split('.').map(|part| {
        part.parse::<u32>()
            .expect("the pinned version is major.minor.patch; a test parses it")
    });
    let mut next = || parts.next().expect("the pinned version has three parts");
    SdkVersion {
        major: next(),
        minor: next(),
        patch: next(),
    }
}

/// Where a file of the archive lands, relative to the SDK directory: its place
/// in the archive without the leading `bin/`, so the library sits beside its
/// `extensions/` folder, which is the layout the loader expects.
#[cfg_attr(
    not(any(feature = "sdk-download", test)),
    expect(dead_code, reason = "only the online build installs from the archive")
)]
pub(super) fn install_path(archive_path: &str) -> &str {
    archive_path.strip_prefix("bin/").unwrap_or(archive_path)
}

#[cfg(test)]
mod tests {
    use odyssey_autocalib::orbbec::LIBRARY_NAME;

    use super::*;

    #[test]
    fn the_pin_is_complete_and_installs_the_library_where_the_loader_looks() {
        let pin = pin();
        assert_ne!(pin.version, "");
        for hash in [&pin.eula.sha256, &pin.windows_x64.sha256] {
            assert_eq!(hash.len(), 64, "sha256 is 64 hex digits: {hash}");
            assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{hash}");
        }
        assert!(
            pin.windows_x64.url.contains(&pin.version),
            "the archive is the pinned version"
        );
        assert!(
            pin.eula.url.contains(&pin.version),
            "the licence is the pinned version's"
        );

        let installed: Vec<&str> = pin
            .windows_x64
            .files
            .iter()
            .map(|f| install_path(f))
            .collect();
        assert!(installed.contains(&"OrbbecSDK.dll"));
        assert!(
            installed.iter().any(|p| p.starts_with("extensions/")),
            "the library needs its extensions folder beside it"
        );
        let unique: std::collections::HashSet<_> = installed.iter().collect();
        assert_eq!(
            unique.len(),
            installed.len(),
            "two archive files would overwrite each other"
        );
        assert!(
            installed
                .iter()
                .all(|p| !p.contains('\\') && !p.starts_with('/') && !p.contains(".."))
        );
    }

    #[test]
    fn the_pinned_version_is_one_the_loader_accepts() {
        let pinned = pinned_version();
        assert!(
            pinned.is_supported(),
            "the app must be tested with a version it accepts: {pinned}"
        );
        assert_eq!(pinned.to_string(), pin().version);
    }

    #[test]
    fn the_pinned_library_name_is_what_the_loader_loads_on_windows() {
        if cfg!(windows) {
            assert_eq!(LIBRARY_NAME, "OrbbecSDK.dll");
            assert!(
                pin()
                    .windows_x64
                    .files
                    .iter()
                    .any(|f| install_path(f) == LIBRARY_NAME)
            );
        }
    }
}
