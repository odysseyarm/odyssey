use std::io::ErrorKind;

use dioxus::desktop::window;
use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_client::client::Client;
use odyssey_hub_common::config::ConfigCorruptionEvent;
use odyssey_hub_server::Message;
use tokio::sync::mpsc::UnboundedReceiver;

/// Why the status stream ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServerExit {
    /// Another app instance owns the hub socket; this one should close.
    AnotherInstanceRunning,
    /// The server stopped, failed to start, or hung up.
    Stopped,
}

/// Asks the instance that already owns the hub socket to raise its window.
async fn bring_existing_instance_to_front() {
    let mut client = Client::default();
    if client.connect().await.is_err() {
        return;
    }
    match client.bring_to_front().await {
        Ok(()) => tracing::info!("Successfully brought existing instance to front"),
        Err(_) => tracing::warn!("Failed to bring existing instance to front"),
    }
}

fn report_corruptions(corruptions: &[ConfigCorruptionEvent]) {
    tracing::warn!(
        "Server started with {} corrupted config file(s)",
        corruptions.len()
    );
    for corruption in corruptions {
        tracing::warn!(
            "Config file corrupted: {:?} at '{}': {}. Using default values.",
            corruption.file_type,
            corruption.file_path,
            corruption.error_message
        );
    }
}

/// Applies the hub server's status messages to the UI until it stops.
pub(crate) async fn handle_server_status(
    mut receiver: UnboundedReceiver<Message>,
    mut config_corruptions: Signal<Vec<ConfigCorruptionEvent>>,
    mut server_ready: Signal<bool>,
) -> ServerExit {
    loop {
        match receiver.recv().await {
            Some(Message::ServerInit(Ok(corruptions))) => {
                if corruptions.is_empty() {
                    tracing::info!("Server started successfully");
                } else {
                    report_corruptions(&corruptions);
                    // A non-empty list opens the config-corruption dialog.
                    config_corruptions.set(corruptions);
                }
                server_ready.set(true);
            }
            Some(Message::ServerInit(Err(e))) => {
                if e.kind() == ErrorKind::AddrInUse {
                    tracing::warn!("Another instance is already running. Bringing it to front...");
                    bring_existing_instance_to_front().await;
                    return ServerExit::AnotherInstanceRunning;
                }
                tracing::error!("Server start error: {}", e);
                return ServerExit::Stopped;
            }
            Some(Message::BringToFront) => {
                tracing::info!("Received BringToFront request");
                window().set_focus();
                window().set_visible(true);
            }
            Some(Message::Stop) => {
                tracing::info!("Server stopped");
                return ServerExit::Stopped;
            }
            None => {
                tracing::info!("Server channel closed");
                return ServerExit::Stopped;
            }
        }
    }
}
