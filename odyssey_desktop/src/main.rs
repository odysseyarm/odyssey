#![windows_subsystem = "windows"]

use dioxus::{
    desktop::{window, Config, WindowBuilder, WindowCloseBehaviour},
    logger::tracing,
    prelude::*,
};
use dioxus_router::{Routable, Router};
use odyssey_hub_server::Message;

use tokio_util::sync::CancellationToken;

use components::Navbar;
use views::Home;
use views::Accessories;

mod components;
mod hub;
mod tray;
mod views;
mod styles;

fn main() {
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
        // Set the close behavior for the main window
        // This will hide the window instead of closing it when the user clicks the close button
        window().set_close_behavior(WindowCloseBehaviour::WindowHides);
    });

    tray::use_tray_menu(cancel_token.clone());

    use_future({
        let cancel_token = cancel_token.clone();
        move || {
            let cancel_token = cancel_token.clone();
            tokio::spawn({
                async move {
                    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
                    tokio::select! {
                        _ = tokio::spawn(odyssey_hub_server::run_server(sender, cancel_token)) => {},
                        _ = tokio::spawn(handle_server_status(receiver)) => {},
                    }
                }
            })
        }
    });

    let hub = use_context_provider(|| Signal::new(hub::HubContext::new()));
    use_future(move || {
        let mut hub = hub();
        async move { hub.run().await }
    });

    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }

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
