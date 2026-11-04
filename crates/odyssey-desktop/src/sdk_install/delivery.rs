use super::locations::managed_dir;

/// How Setup offers a missing SDK on this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// The app downloads it after the user accepts the licence.
    Download,
    /// This build never downloads the camera SDK; the installer carries it as an option.
    Installer,
    /// Offline mode is on: nothing is downloaded and no web link is offered.
    Offline,
    /// Nothing is automated here: point at Orbbec's download page.
    Link,
}

pub(crate) fn delivery() -> Delivery {
    // Offline mode comes before everything: it forbids the download and the link.
    if odyssey_hub_server::policy::offline() {
        return Delivery::Offline;
    }
    let windows = cfg!(all(windows, target_arch = "x86_64"));
    if !windows {
        Delivery::Link
    } else if cfg!(feature = "sdk-download") && managed_dir().is_some() {
        Delivery::Download
    } else {
        Delivery::Installer
    }
}
