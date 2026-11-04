//! The firmware state the Home page shares with every device row.

use std::time::Duration;

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_server::firmware::{self, FirmwareManifest};

use crate::components::FirmwareUpdateManager;
use crate::components::firmware_update::{FIRMWARE_CHANNEL, ensure_firmware_channel_loaded};

/// How often the list of devices being updated is refreshed.
const UPDATING_POLL_INTERVAL: Duration = Duration::from_millis(200);
/// The first wait after a failed manifest fetch; each retry doubles it.
const MANIFEST_RETRY_START: Duration = Duration::from_secs(5);
const MANIFEST_RETRY_MAX: Duration = Duration::from_secs(300);

/// The firmware manifest, fetched once and shared across all device firmware
/// update components, and why there is none, for the offline notice.
#[derive(Clone, Copy)]
pub(super) struct FirmwareManifestState {
    pub(super) manifest: Signal<Option<FirmwareManifest>>,
    pub(super) error: Signal<Option<String>>,
}

/// Fetches the manifest for the selected channel, again whenever the channel
/// changes.
///
/// Without the manifest no update is ever offered, and one failed fetch used
/// to disable updates for the rest of the session. This keeps trying, backing
/// off so an offline machine is not hammering the network.
pub(super) fn use_firmware_manifest() -> FirmwareManifestState {
    let mut manifest: Signal<Option<FirmwareManifest>> = use_signal(|| None);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    use_future(ensure_firmware_channel_loaded);
    use_resource(move || async move {
        // Reading the signal here re-runs this when Settings changes it.
        let Some(channel) = FIRMWARE_CHANNEL() else {
            return;
        };
        manifest.set(None);
        error.set(None);
        let mut delay = MANIFEST_RETRY_START;
        loop {
            match firmware::fetch_manifest(channel).await {
                Ok(fetched) => {
                    tracing::info!(
                        "Fetched {} firmware manifest version {}",
                        channel.name(),
                        fetched.version
                    );
                    error.set(None);
                    manifest.set(Some(fetched));
                    return;
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    tracing::warn!("Failed to fetch firmware manifest, retrying: {}", e);
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(MANIFEST_RETRY_MAX);
                }
            }
        }
    });

    FirmwareManifestState { manifest, error }
}

/// The uuids of the devices being updated, refreshed while the page is open.
/// The signal also re-renders the page when updates start and finish.
pub(super) fn use_updating_uuids(manager: &FirmwareUpdateManager) -> Signal<Vec<[u8; 6]>> {
    let mut updating_uuids = use_signal(Vec::<[u8; 6]>::new);
    use_future({
        let manager = manager.clone();
        move || {
            let manager = manager.clone();
            async move {
                loop {
                    tokio::time::sleep(UPDATING_POLL_INTERVAL).await;
                    updating_uuids.set(manager.get_all_updating());
                }
            }
        }
    });
    updating_uuids
}
