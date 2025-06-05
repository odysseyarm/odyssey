use dioxus::{
    desktop::{Config, WindowBuilder},
    logger::tracing,
    prelude::*,
};
use dioxus_router::prelude::*;
use odyssey_hub_server::Message;

use tokio_util::sync::CancellationToken;

use components::Navbar;
use views::Home;

mod components;
mod hub;
mod tray;
mod views;

fn main() {
    if cfg!(target_os = "windows") {
        let user_data_dir = std::env::var("LOCALAPPDATA").expect("env var LOCALAPPDATA not found");
        dioxus::LaunchBuilder::new()
            .with_cfg(
                Config::default()
                    .with_data_directory(user_data_dir)
                    .with_menu(None)
                    .with_close_behaviour(dioxus::desktop::WindowCloseBehaviour::LastWindowHides)
                    .with_window(WindowBuilder::new().with_title("Odyssey")),
            )
            .launch(app);
    } else {
        dioxus::LaunchBuilder::new()
            .with_cfg(
                Config::default()
                    .with_menu(None)
                    .with_close_behaviour(dioxus::desktop::WindowCloseBehaviour::LastWindowHides)
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
}

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
fn app() -> Element {
    tray::init();

    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let cancel_token = CancellationToken::new();

    tokio::spawn(async {
        tokio::select! {
            _ = tokio::spawn(odyssey_hub_server::run_server(sender, cancel_token)) => {},
            _ = tokio::spawn(handle_server_status(receiver)) => {},
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
