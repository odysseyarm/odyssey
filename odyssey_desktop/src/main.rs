#![windows_subsystem = "windows"]

use dioxus::{
    desktop::{window, Config, WindowBuilder, WindowCloseBehaviour},
    logger::tracing,
    prelude::*,
};
use dioxus_router::{Routable, Router};
use odyssey_hub_server::Message;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use velopack::VelopackApp;

use components::Navbar;
use views::Accessories;
use views::Home;

use components::update::UpdateBanner;

mod components;
mod hub;
mod styles;
mod tray;
mod views;

fn main() {
    VelopackApp::build().run();

    if cfg!(target_os = "windows") {
        let user_data_dir = std::env::var("LOCALAPPDATA").expect("env var LOCALAPPDATA not found");
        dioxus::LaunchBuilder::new()
            .with_cfg(
                Config::default()
                    .with_data_directory(user_data_dir)
                    .with_menu(None)
                    .with_close_behaviour(dioxus::desktop::WindowCloseBehaviour::WindowCloses)
                    .with_window(WindowBuilder::new().with_title("Odyssey")),
            )
            .launch(app);
    } else {
        dioxus::LaunchBuilder::new()
            .with_cfg(
                Config::default()
                    .with_menu(None)
                    .with_close_behaviour(dioxus::desktop::WindowCloseBehaviour::WindowCloses)
                    .with_window(WindowBuilder::new().with_title("Odyssey")),
            )
            .launch(app);
    }
}

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[layout(Navbar)]
    #[route("/")]
    Home {},
    #[route("/accessories")]
    Accessories {},
}

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
fn app() -> Element {
    let cancel_token = CancellationToken::new();

    use_hook(|| {
        // Hide window to tray on close
        window().set_close_behavior(WindowCloseBehaviour::WindowHides);
    });

    tray::use_tray_menu(cancel_token.clone());

    // Run server + status handling
    use_future({
        let cancel_token = cancel_token.clone();
        move || {
            let cancel_token = cancel_token.clone();
            tokio::spawn(async move {
                use futures::stream::{self, StreamExt};
                use futures_concurrency::prelude::*;

                let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

                // Convert server task to stream
                let server_stream = stream::once({
                    let sender = sender.clone();
                    let cancel_token = cancel_token.clone();
                    async move {
                        let _ = odyssey_hub_server::run_server(sender, cancel_token).await;
                        0u8
                    }
                });

                // Convert status handler to stream
                let status_stream = stream::once(async move {
                    handle_server_status(receiver).await;
                    1u8
                });

                // Merge both streams
                let merged = (server_stream, status_stream).merge();
                tokio::pin!(merged);

                // Wait for either to complete
                while let Some(_) = merged.next().await {
                    break;
                }
            })
        }
    });

    let hub = use_context_provider(|| Signal::new(hub::HubContext::new()));
    use_future(move || {
        let mut hub = hub();
        async move { hub.run().await }
    });

    // --- Update banner signals + one-shot check ---
    let update_available = use_signal(|| false);
    let update_busy = use_signal(|| false);
    let update_error = use_signal(|| Option::<String>::None);

    // One-shot update check
    use_effect({
        let mut available = update_available.clone();
        let mut error = update_error.clone();
        move || {
            dioxus::prelude::spawn(async move {
                use velopack::{self as vp, sources};

                let source = sources::HttpSource::new(
                    "https://github.com/odysseyarm/odyssey-desktop/releases/latest/download",
                );
                match vp::UpdateManager::new(source, None, None) {
                    Ok(um) => {
                        match um.check_for_updates_async().await {
                            Ok(vp::UpdateCheck::UpdateAvailable(_)) => {
                                tracing::info!("Update available");
                                available.set(true);
                            }
                            Ok(_) => {
                                tracing::info!("No update available");
                                // no update available
                            }
                            Err(e) => {
                                tracing::error!("Error checking for updates: {e}");
                                error.set(Some(format!("{e}")));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error creating UpdateManager: {e}");
                        error.set(Some(format!("{e}")));
                    }
                }
            });
        }
    });

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }

        UpdateBanner {
            available: update_available,
            busy: update_busy,
            error: update_error,
            cancel_token: Signal::new(cancel_token.clone()),
        }

        div {
            Router::<Route> {}
        }
    }
}

async fn handle_server_status(mut receiver: tokio::sync::mpsc::UnboundedReceiver<Message>) {
    loop {
        match receiver.recv().await {
            Some(Message::ServerInit(Ok(()))) => {
                tracing::info!("Server started");
            }
            Some(Message::ServerInit(Err(_))) => {
                tracing::error!("Server start error");
                break;
            }
            Some(Message::Stop) => {
                tracing::info!("Server stopped");
                break;
            }
            None => {
                tracing::info!("Server channel closed");
                break;
            }
        }
    }
}
