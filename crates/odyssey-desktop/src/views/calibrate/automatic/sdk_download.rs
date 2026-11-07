//! Offers to download Orbbec's camera software, after the user has accepted
//! Orbbec's licence, to remove a copy that was downloaded, and to say when the
//! copy in use is not the version this app is tested with.
//!
//! Only builds with the `sdk-download` feature contain the download. Builds for
//! customers who may only run offline software leave it out, and
//! `sdk_install::delivery` tells Setup to point at the installer instead.

use gpui::{
    AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};

use super::wizard::{CameraList, Wizard};
use crate::components::ui::{AlertKind, ButtonKind, alert, button};
use crate::sdk_install::{self, Advisory, Delivery, Removal};
use crate::theme::palette;

#[cfg(feature = "sdk-download")]
mod online {
    use gpui::component::Disableable as _;
    use gpui::component::checkbox::Checkbox;
    use gpui::{
        Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
        Window, div, px,
    };

    use super::super::ui::{link_button, working_row};
    use super::super::wizard::Wizard;
    use crate::components::ui::{ButtonKind, button};
    use crate::sdk_install::{self, Installed, Phase};
    use crate::theme::palette;

    #[derive(Clone, PartialEq)]
    enum State {
        Ready,
        Working(Phase),
        /// Downloaded, but the copy in use cannot be replaced until Odyssey restarts.
        Waiting,
        Failed(String),
    }

    /// What the progress line says.
    fn describe(phase: Phase) -> String {
        match phase {
            Phase::Downloading { received, total } if total > 0 => {
                format!("Downloading… {}%", received.saturating_mul(100) / total)
            }
            Phase::Downloading { .. } => "Downloading…".to_string(),
            Phase::Verifying => "Checking the download…".to_string(),
            Phase::Installing => "Installing…".to_string(),
        }
    }

    pub(in super::super) struct DownloadSdk {
        wizard: Entity<Wizard>,
        accepted: bool,
        state: State,
    }

    impl DownloadSdk {
        pub(in super::super) fn new(wizard: Entity<Wizard>, _: &mut Context<Self>) -> Self {
            Self {
                wizard,
                accepted: false,
                state: State::Ready,
            }
        }

        fn start(&mut self, cx: &mut Context<Self>) {
            self.state = State::Working(Phase::Downloading {
                received: 0,
                total: 0,
            });
            cx.notify();
            let (progress, mut phases) = tokio::sync::mpsc::unbounded_channel();
            let job = crate::runtime::spawn(async move {
                sdk_install::install(|phase| {
                    let _ = progress.send(phase);
                })
                .await
                .map_err(|error| crate::error_message(&error))
            });
            cx.spawn(async move |this, cx| {
                // The channel closes when `install` finishes and drops its callback.
                while let Some(phase) = phases.recv().await {
                    let alive = this.update(cx, |this, cx| {
                        this.state = State::Working(phase);
                        cx.notify();
                    });
                    if alive.is_err() {
                        return;
                    }
                }
                let result = job
                    .await
                    .unwrap_or_else(|error| Err(format!("the download task stopped: {error}")));
                let _ = this.update(cx, |this, cx| {
                    this.state = match result {
                        Ok(Installed::Ready) => {
                            this.wizard
                                .update(cx, |wizard, cx| wizard.request_camera_list(cx));
                            State::Ready
                        }
                        Ok(Installed::AfterRestart) => State::Waiting,
                        Err(error) => State::Failed(error),
                    };
                    cx.notify();
                });
            })
            .detach();
        }
    }

    impl Render for DownloadSdk {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let p = palette(cx);
            let pin = sdk_install::pin();
            let megabytes = pin.windows_x64.size / 1_000_000;
            let working = matches!(self.state, State::Working(_));
            let url = pin.eula.url.clone();

            let status = match &self.state {
                State::Working(phase) => Some(working_row(p, describe(*phase)).into_any_element()),
                State::Waiting => Some(
                    div()
                        .text_sm()
                        .text_color(p.fg)
                        .child("Downloaded. Restart Odyssey to switch to it.")
                        .into_any_element(),
                ),
                State::Failed(error) => Some(
                    div()
                        .text_sm()
                        .text_color(p.danger_ink)
                        .child(format!("The download didn't work: {error}"))
                        .into_any_element(),
                ),
                State::Ready => None,
            };

            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .pt_2()
                .max_w(px(576.0))
                .child(div().text_sm().text_color(p.fg_muted).child(format!(
                    "Odyssey can download Orbbec SDK {} for you (about {megabytes} MB). \
                     Orbbec licenses it for use with Orbbec cameras under their End User License Agreement.",
                    pin.version
                )))
                .child(link_button(
                    "sdk-eula",
                    p,
                    "Read the licence",
                    move |_, _, cx| cx.open_url(&url),
                ))
                .child(
                    Checkbox::new("sdk-accept")
                        .checked(self.accepted)
                        .disabled(working)
                        .label("I accept Orbbec's End User License Agreement")
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.accepted = *checked;
                            cx.notify();
                        })),
                )
                .children(status)
                .child(
                    button(
                        "sdk-download",
                        SharedString::from(format!("Download Orbbec SDK {}", pin.version)),
                        ButtonKind::Primary,
                        p,
                    )
                    .disabled(!self.accepted || working)
                    .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                )
        }
    }
}

#[cfg(feature = "sdk-download")]
pub(super) use online::DownloadSdk;

/// Without the feature there is nothing to download; Setup never asks for this.
#[cfg(not(feature = "sdk-download"))]
pub(super) struct DownloadSdk;

#[cfg(not(feature = "sdk-download"))]
impl DownloadSdk {
    pub(super) fn new(_: Entity<Wizard>, _: &mut Context<Self>) -> Self {
        Self
    }
}

#[cfg(not(feature = "sdk-download"))]
impl Render for DownloadSdk {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// Says so when the camera software in use is not the version this app is tested
/// with. Only a warning: the loader accepts 2.8 and newer, so the cameras still
/// work. When the copy is one Odyssey downloaded and it is older, the pinned one
/// can be downloaded in its place; for a copy found elsewhere, or a newer one,
/// there is nothing to offer.
pub(super) struct SdkVersionNote {
    wizard: Entity<Wizard>,
    download: Entity<DownloadSdk>,
    _observe: Subscription,
}

impl SdkVersionNote {
    pub(super) fn new(wizard: Entity<Wizard>, cx: &mut Context<Self>) -> Self {
        let download = cx.new(|cx| DownloadSdk::new(wizard.clone(), cx));
        let observe = cx.observe(&wizard, |_, _, cx| cx.notify());
        Self {
            wizard,
            download,
            _observe: observe,
        }
    }
}

impl Render for SdkVersionNote {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        // Cameras were listed, so the library is loaded and the lookup below is cached.
        if !matches!(self.wizard.read(cx).cameras, CameraList::Found(_)) {
            return div();
        }
        let Some(advisory) = sdk_install::current_advisory() else {
            return div();
        };

        let (title, text, extra) = match advisory {
            Advisory::Older {
                found,
                pinned,
                replaceable,
            } => {
                let extra = if replaceable && sdk_install::delivery() == Delivery::Download {
                    Some(div().child(self.download.clone()))
                } else if !replaceable {
                    Some(
                        div()
                            .text_sm()
                            .child("Odyssey didn't install it, so update it where you got it."),
                    )
                } else {
                    None
                };
                (
                    "The camera software is older than this app is tested with",
                    format!(
                        "Orbbec SDK {found} is in use and Odyssey is tested with {pinned}. \
                         The camera should still work, but update it if you run into problems."
                    ),
                    extra,
                )
            }
            Advisory::Newer { found, pinned } => (
                "The camera software is newer than this app is tested with",
                format!(
                    "Orbbec SDK {found} is in use and Odyssey is tested with {pinned}. \
                     It will probably work, but if the camera misbehaves, install {pinned}."
                ),
                None,
            ),
        };
        div().child(alert(
            AlertKind::Warning,
            p,
            div()
                .flex()
                .flex_col()
                .gap_2()
                .text_base()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().child(text))
                .children(extra),
        ))
    }
}

/// A line under the camera list for a copy of the SDK this app downloaded, with a
/// way to delete it. Shows nothing otherwise, including for the installer's copy,
/// which Windows removes with the app.
pub(super) struct DownloadedSdkRow {
    wizard: Entity<Wizard>,
    error: Option<String>,
}

impl DownloadedSdkRow {
    pub(super) fn new(wizard: Entity<Wizard>, _: &mut Context<Self>) -> Self {
        Self {
            wizard,
            error: None,
        }
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        self.error = match sdk_install::remove_managed_copy() {
            // Either way the row redraws: gone, or marked as pending.
            Ok(Removal::Removed | Removal::AfterRestart) => None,
            Err(e) => Some(format!("Couldn't remove it: {e}")),
        };
        self.wizard
            .update(cx, |wizard, cx| wizard.request_camera_list(cx));
        cx.notify();
    }
}

impl Render for DownloadedSdkRow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        if sdk_install::delivery() != Delivery::Download || !sdk_install::has_managed_copy() {
            return div();
        }
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_3()
            .text_sm()
            .text_color(p.fg_muted);
        if sdk_install::removal_pending() {
            row = row.child("The camera software Odyssey downloaded will be removed the next time Odyssey starts.");
        } else {
            row = row
                .child(if sdk_install::update_pending() {
                    "Camera software downloaded by Odyssey. A newer copy takes over the next time Odyssey starts."
                } else {
                    "Camera software downloaded by Odyssey."
                })
                .child(
                    button("sdk-remove", "Remove", ButtonKind::Secondary, p)
                        .on_click(cx.listener(|this, _, _, cx| this.remove(cx))),
                );
        }
        if let Some(message) = &self.error {
            row = row.child(
                div()
                    .text_color(p.danger_ink)
                    .child(SharedString::from(message.clone())),
            );
        }
        row
    }
}
