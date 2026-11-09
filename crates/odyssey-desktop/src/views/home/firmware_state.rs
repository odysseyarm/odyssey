//! The firmware manifest the Home page shares with every device row.

use std::time::Duration;

use backon::{ExponentialBuilder, Retryable as _};
use gpui::{Context, Task};
use odyssey_hub_server::firmware::{self, FirmwareChannel, FirmwareManifest};
use tokio::sync::mpsc;

use crate::components::firmware_update::FirmwareChannelModel;
use crate::runtime;

/// The first wait after a failed manifest fetch; each retry doubles it.
const MANIFEST_RETRY_START: Duration = Duration::from_secs(5);
const MANIFEST_RETRY_MAX: Duration = Duration::from_secs(300);

enum Fetch {
    Failed(String),
    Done(FirmwareManifest),
}

/// The firmware manifest for the selected channel, shared across all device
/// firmware update rows, and why there is none, for the offline notice.
///
/// Without the manifest no update is ever offered, so a failed fetch is
/// retried for the whole session, backing off so an offline machine does not
/// hammer the network. It is fetched again whenever the channel changes.
pub(super) struct FirmwareManifestModel {
    pub(super) manifest: Option<FirmwareManifest>,
    pub(super) error: Option<String>,
    channel: Option<FirmwareChannel>,
    fetcher: Option<tokio::task::JoinHandle<()>>,
    receiver_task: Option<Task<()>>,
    _channel_subscription: gpui::Subscription,
}

impl FirmwareManifestModel {
    pub(super) fn new(cx: &mut Context<'_, Self>) -> Self {
        let channel_model = FirmwareChannelModel::global(cx);
        let subscription = cx.observe(&channel_model, |this, model, cx| {
            let channel = model.read(cx).channel();
            if channel != this.channel {
                this.restart(channel, cx);
            }
        });
        let mut this = Self {
            manifest: None,
            error: None,
            channel: None,
            fetcher: None,
            receiver_task: None,
            _channel_subscription: subscription,
        };
        this.restart(channel_model.read(cx).channel(), cx);
        this
    }

    fn restart(&mut self, channel: Option<FirmwareChannel>, cx: &mut Context<'_, Self>) {
        self.stop();
        self.channel = channel;
        self.manifest = None;
        self.error = None;
        cx.notify();
        let Some(channel) = channel else {
            return;
        };

        let (sender, mut receiver) = mpsc::unbounded_channel();
        let backoff = ExponentialBuilder::new()
            .with_factor(2.0)
            .with_min_delay(MANIFEST_RETRY_START)
            .with_max_delay(MANIFEST_RETRY_MAX)
            .without_max_times();
        self.fetcher = Some(runtime::spawn(async move {
            let fetched = (|| firmware::fetch_manifest(channel))
                .retry(backoff)
                .notify(|e, _| {
                    tracing::warn!("Failed to fetch firmware manifest, retrying: {}", e);
                    let _ = sender.send(Fetch::Failed(e.to_string()));
                })
                .await;
            if let Ok(fetched) = fetched {
                tracing::info!(
                    "Fetched {} firmware manifest version {}",
                    channel.name(),
                    fetched.version
                );
                let _ = sender.send(Fetch::Done(fetched));
            }
        }));
        self.receiver_task = Some(cx.spawn(async move |this, cx| {
            while let Some(fetch) = receiver.recv().await {
                let applied = this.update(cx, |this, cx| {
                    match fetch {
                        Fetch::Failed(error) => this.error = Some(error),
                        Fetch::Done(manifest) => {
                            this.error = None;
                            this.manifest = Some(manifest);
                        }
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        }));
    }

    fn stop(&mut self) {
        drop(self.receiver_task.take());
        if let Some(fetcher) = self.fetcher.take() {
            fetcher.abort();
        }
    }
}

impl Drop for FirmwareManifestModel {
    fn drop(&mut self) {
        self.stop();
    }
}
