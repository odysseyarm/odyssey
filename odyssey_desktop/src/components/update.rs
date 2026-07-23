use std::path::PathBuf;

use dioxus::prelude::*;
use tokio_util::sync::CancellationToken;
use velopack::{self as vp, sources, UpdateManager, UpdateOptions};

pub const REPO_URL: &str = "https://github.com/odysseyarm/odyssey-desktop";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
    Nightly,
}

impl UpdateChannel {
    pub const ALL: [UpdateChannel; 3] = [Self::Stable, Self::Beta, Self::Nightly];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Nightly => "nightly",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stable => "Stable",
            Self::Beta => "Beta",
            Self::Nightly => "Nightly",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "stable" => Some(Self::Stable),
            "beta" => Some(Self::Beta),
            "nightly" => Some(Self::Nightly),
            _ => None,
        }
    }

    pub fn velopack_channel(self) -> String {
        format!("win-x64-{}", self.as_str())
    }

    /// Beta and nightly packages live on GitHub releases marked as prereleases.
    pub fn includes_prereleases(self) -> bool {
        !matches!(self, Self::Stable)
    }
}

fn channel_file() -> Option<PathBuf> {
    crate::paths::settings_dir().map(|dir| dir.join("update_channel.txt"))
}

pub fn load_channel() -> UpdateChannel {
    channel_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|s| UpdateChannel::parse(s.trim()))
        .unwrap_or_default()
}

pub fn save_channel(channel: UpdateChannel) {
    if let Some(path) = channel_file() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, channel.as_str());
    }
}

pub fn update_manager(channel: UpdateChannel) -> Result<UpdateManager, vp::Error> {
    let source = sources::GithubSource::new(REPO_URL, None, channel.includes_prereleases());
    let options = UpdateOptions {
        ExplicitChannel: Some(channel.velopack_channel()),
        // Channel switches can legitimately move to an older version
        // (e.g. nightly -> stable).
        AllowVersionDowngrade: true,
        ..Default::default()
    };
    UpdateManager::new(source, Some(options), None)
}

/// Shared update state, provided as a context from the app root.
#[derive(Clone, Copy)]
pub struct UpdateState {
    pub channel: Signal<UpdateChannel>,
    pub available: Signal<bool>,
    pub busy: Signal<bool>,
    pub error: Signal<Option<String>>,
    pub checking: Signal<bool>,
    /// Bumped to request a re-check (the root effect tracks it).
    pub check_tick: Signal<u32>,
}

impl UpdateState {
    pub fn new_in_scope() -> Self {
        Self {
            channel: Signal::new(load_channel()),
            available: Signal::new(false),
            busy: Signal::new(false),
            error: Signal::new(None),
            checking: Signal::new(false),
            check_tick: Signal::new(0),
        }
    }

    pub fn request_check(&mut self) {
        let tick = *self.check_tick.peek() + 1;
        self.check_tick.set(tick);
    }
}

#[component]
pub fn UpdateChannelPicker() -> Element {
    let mut channel = use_context::<UpdateState>().channel;

    rsx! {
        div {
            label {
                class: "block text-xs text-gray-500 dark:text-gray-400 mb-1",
                "Update channel"
            }
            select {
                class: "w-full text-sm rounded bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-white p-1",
                value: "{channel().as_str()}",
                onchange: move |e| {
                    if let Some(ch) = UpdateChannel::parse(&e.value()) {
                        channel.set(ch);
                        save_channel(ch);
                    }
                },
                for ch in UpdateChannel::ALL {
                    option {
                        value: "{ch.as_str()}",
                        selected: channel() == ch,
                        "{ch.label()}"
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct UpdateBannerProps {
    pub cancel_token: Signal<CancellationToken>,
}

#[component]
pub fn UpdateBanner(props: UpdateBannerProps) -> Element {
    let state = use_context::<UpdateState>();
    let channel = state.channel;

    // If no update, render nothing
    if !(state.available)() {
        return rsx! {};
    }

    let on_update_click = {
        // mutate these signals in the async task
        let mut available = state.available;
        let mut busy = state.busy;
        let mut error = state.error;

        move |_| {
            if (busy)() {
                return;
            }
            busy.set(true);
            error.set(None);

            let channel = *channel.peek();
            let cancel_token = props.cancel_token.peek().clone();
            dioxus::prelude::spawn(async move {
                // velopack 1.x is sync; keep network + apply off the UI executor
                let result = tokio::task::spawn_blocking(move || {
                    let um = update_manager(channel).map_err(|e| format!("{e}"))?;
                    match um.check_for_updates().map_err(|e| format!("{e}"))? {
                        vp::UpdateCheck::UpdateAvailable(info) => {
                            um.download_updates(&info, None)
                                .map_err(|e| format!("{e}"))?;
                            cancel_token.cancel();
                            // TODO wait on server shutdown
                            um.apply_updates_and_restart(&info.TargetFullRelease)
                                .map_err(|e| format!("{e}"))?;
                            Ok(true)
                        }
                        _ => Ok(false),
                    }
                })
                .await
                .unwrap_or_else(|e| Err(format!("{e}")));

                match result {
                    // Updated: the app is restarting, nothing left to do
                    Ok(true) => {}
                    Ok(false) => {
                        available.set(false);
                        busy.set(false);
                    }
                    Err(e) => {
                        error.set(Some(e));
                        busy.set(false);
                    }
                }
            });
        }
    };

    rsx! {
        // Fixed banner at the top
        div { class: "fixed top-0 left-0 right-0 z-50",
            div { class: "bg-red-600 text-white px-4 py-2 flex items-center justify-between gap-3",
                span { class: "font-medium",
                    "Odyssey is out of date. Update and restart?"
                }
                div { class: "flex items-center gap-2",
                    if let Some(err) = (state.error)() {
                        span { class: "text-white/90 text-sm", "{err}" }
                    }
                    button {
                        class: "bg-white text-red-700 rounded px-3 py-1 text-sm font-semibold hover:bg-red-50 disabled:opacity-60",
                        disabled: (state.busy)(),
                        onclick: on_update_click,
                        if (state.busy)() { "Updating…" } else { "Update & Restart" }
                    }
                    button {
                        class: "bg-transparent border border-white/70 text-white rounded px-3 py-1 text-sm hover:bg-white/10",
                        disabled: (state.busy)(),
                        onclick: {
                            let mut available = state.available;
                            move |_| available.set(false)
                        },
                        "Later"
                    }
                }
            }
            // Spacer so content isn't hidden by fixed banner
            div { class: "h-9" }
        }
    }
}
