use gpui::component::radio::Radio;
use gpui::{
    Context, Entity, FontWeight, IntoElement, ParentElement as _, Render, Styled as _, Window, div,
};
use odyssey_hub_server::firmware::FirmwareChannel;

use crate::components::firmware_update::FirmwareChannelModel;
use crate::components::ui::{card, card_title, page};
use crate::runtime;
use crate::theme::palette;

const CHANNELS: [(FirmwareChannel, &str, &str); 2] = [
    (
        FirmwareChannel::Stable,
        "Stable",
        "Firmware that has been tested and promoted. Recommended.",
    ),
    (
        FirmwareChannel::Nightly,
        "Nightly",
        "Every firmware release as soon as it is published. May contain unfinished changes.",
    ),
];

pub(crate) struct SettingsView {
    channel: Entity<FirmwareChannelModel>,
    // The choice applies at once; a failed save only means it is lost at the
    // next start, which the user has to be told.
    save_error: Option<String>,
}

impl SettingsView {
    pub(crate) fn new(_window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let channel = FirmwareChannelModel::global(cx);
        cx.observe(&channel, |_, _, cx| cx.notify()).detach();
        Self {
            channel,
            save_error: None,
        }
    }

    fn choose(&mut self, channel: FirmwareChannel, cx: &mut Context<'_, Self>) {
        self.channel.update(cx, |model, cx| model.set(channel, cx));
        let save =
            runtime::spawn(async move { channel.save().await.map_err(|error| error.to_string()) });
        cx.spawn(async move |this, cx| {
            let outcome = save
                .await
                .map_err(|error| error.to_string())
                .and_then(std::convert::identity);
            if let Err(error) = &outcome {
                tracing::warn!("Could not save the firmware channel: {error}");
            }
            let _ = this.update(cx, |this, cx| {
                this.save_error = outcome.err();
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let p = palette(cx);
        let current = self.channel.read(cx).channel().unwrap_or_default();

        let mut updates = card(p).gap_4().child(card_title(p, "Firmware updates"));
        for (channel, label, help) in CHANNELS {
            updates = updates.child(
                Radio::new(label)
                    .checked(current == channel)
                    .accessibility_label(label)
                    .on_click(cx.listener(move |this, _: &bool, _, cx| this.choose(channel, cx)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.fg_strong)
                                    .child(label),
                            )
                            .child(div().text_sm().text_color(p.fg_muted).child(help)),
                    ),
            );
        }
        if let Some(error) = &self.save_error {
            updates = updates.child(div().text_sm().text_color(p.danger_ink).child(format!(
                "Applied for now, but could not be saved, so it resets when the app restarts: {error}"
            )));
        }

        let about = card(p).gap_1().child(card_title(p, "About")).child(
            div().text_sm().text_color(p.fg_muted).child(format!(
                "Odyssey Desktop v{}",
                env!("ODYSSEY_BUILD_VERSION")
            )),
        );

        page(
            p,
            "Settings",
            None,
            [updates.into_any_element(), about.into_any_element()],
        )
    }
}
