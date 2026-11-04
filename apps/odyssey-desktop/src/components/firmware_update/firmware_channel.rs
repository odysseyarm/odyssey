use dioxus::prelude::*;
use odyssey_hub_server::firmware;

/// The firmware channel in effect, `None` until the saved choice has loaded.
/// Settings writes it and the manifest fetch follows it, so a change applies
/// without a restart.
pub(crate) static FIRMWARE_CHANNEL: GlobalSignal<Option<firmware::FirmwareChannel>> =
    Signal::global(|| None);

/// Fill [`FIRMWARE_CHANNEL`] from the saved choice if nothing has set it yet.
///
/// The check repeats after the load: a user can pick a channel while the saved
/// one is still being read, and that pick must survive.
pub(crate) async fn ensure_firmware_channel_loaded() {
    if FIRMWARE_CHANNEL.peek().is_some() {
        return;
    }
    let saved = firmware::FirmwareChannel::load().await;
    let mut channel = FIRMWARE_CHANNEL.write();
    if channel.is_none() {
        *channel = Some(saved);
    }
}
