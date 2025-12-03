use odyssey_hub_server::{self, Message};
use tokio_util::sync::CancellationToken;
use tracing::Level;
use tracing_subscriber::EnvFilter;

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

    // Spawn server in background
    let server_handle = tokio::spawn(odyssey_hub_server::run_server(sender, cancel_token));

    // Handle status messages
    handle_server_status(receiver).await;

    // Wait for server task to complete
    let result = server_handle.await;
    println!("Server exited with: {:?}", result);
}

async fn handle_server_status(mut receiver: tokio::sync::mpsc::UnboundedReceiver<Message>) {
    loop {
        match receiver.recv().await {
            Some(Message::ServerInit(Ok(_))) => {
                tracing::info!("Server started");
            }
            Some(Message::ServerInit(Err(e))) => {
                // Check if another instance is already running
                if e.kind() == std::io::ErrorKind::AddrInUse {
                    tracing::warn!("Another instance is already running. Notifying it to bring window to front...");

                    // Try to connect to the existing instance and call BringToFront
                    let mut client = odyssey_hub_client::client::Client::default();
                    if let Ok(()) = client.connect().await {
                        if let Ok(()) = client.bring_to_front().await {
                            tracing::info!("Successfully notified existing instance");
                        } else {
                            tracing::warn!("Failed to notify existing instance");
                        }
                    }
                } else {
                    tracing::error!("Server start error: {}", e);
                }
                break;
            }
            Some(Message::Stop) => {
                tracing::info!("Server stopped");
                break;
            }
            Some(Message::BringToFront) => {
                tracing::info!("Bring to front (no GUI in standalone mode)");
            }
            None => {
                tracing::info!("Server channel closed");
                break;
            }
        }
    }
}
