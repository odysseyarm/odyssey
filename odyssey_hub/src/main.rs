use futures::stream::{self, StreamExt};
use futures_concurrency::prelude::*;
use odyssey_hub_server::{self, Message};
use tokio_util::sync::CancellationToken;
use tracing::Level;
use tracing_subscriber::EnvFilter;

/// Events from the main merged stream
enum MainEvent {
    ServerExited,
    StatusHandlerExited,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_env_var("RUST_LOG")
                .with_default_directive(Level::INFO.into())
                .from_env_lossy(),
        )
        .init();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let cancel_token = CancellationToken::new();
    tokio::spawn({
        let cancel_token = cancel_token.clone();
        async move {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to listen for ctrl-c");
            cancel_token.cancel();
        }
    });

    // Convert server task to stream
    let server_stream = stream::once(async move {
        let result = tokio::spawn(odyssey_hub_server::run_server(sender, cancel_token)).await;
        println!("Server exited with: {:?}", result);
        MainEvent::ServerExited
    });

    // Convert status handler to stream
    let status_stream = stream::once(async move {
        handle_server_status(receiver).await;
        MainEvent::StatusHandlerExited
    });

    // Merge both streams
    let merged = (server_stream, status_stream).merge();
    tokio::pin!(merged);

    // Wait for either to complete
    while let Some(_event) = merged.next().await {
        break;
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
