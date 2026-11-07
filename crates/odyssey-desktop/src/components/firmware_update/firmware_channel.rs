use gpui::{App, AppContext as _, Context, Entity, Global};
use odyssey_hub_server::firmware::FirmwareChannel;

use crate::runtime;

/// The firmware channel in effect, `None` until the saved choice has loaded.
/// Settings writes it and the manifest fetch follows it (by observing the
/// entity), so a change applies without a restart.
pub(crate) struct FirmwareChannelModel {
    channel: Option<FirmwareChannel>,
}

struct FirmwareChannelHandle(Entity<FirmwareChannelModel>);

impl Global for FirmwareChannelHandle {}

impl FirmwareChannelModel {
    /// The app-wide model; the first call creates it and starts loading the
    /// saved choice.
    pub(crate) fn global(cx: &mut App) -> Entity<Self> {
        if let Some(handle) = cx.try_global::<FirmwareChannelHandle>() {
            return handle.0.clone();
        }
        let model = cx.new(|cx: &mut Context<'_, Self>| {
            let load = runtime::spawn(FirmwareChannel::load());
            cx.spawn(async move |this, cx| {
                let Ok(saved) = load.await else { return };
                this.update(cx, |this, cx| this.set_if_unset(saved, cx))
                    .ok();
            })
            .detach();
            Self { channel: None }
        });
        cx.set_global(FirmwareChannelHandle(model.clone()));
        model
    }

    pub(crate) fn channel(&self) -> Option<FirmwareChannel> {
        self.channel
    }

    /// Applies `channel` at once. Persisting it is the caller's job.
    pub(crate) fn set(&mut self, channel: FirmwareChannel, cx: &mut Context<'_, Self>) {
        self.channel = Some(channel);
        cx.notify();
    }

    /// A user can pick a channel while the saved one is still being read, and
    /// that pick must survive.
    fn set_if_unset(&mut self, saved: FirmwareChannel, cx: &mut Context<'_, Self>) {
        if self.channel.is_none() {
            self.set(saved, cx);
        }
    }
}
