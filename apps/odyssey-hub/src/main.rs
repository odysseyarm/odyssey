use std::process::ExitCode;

use odyssey_hub_server::{self, Message};
use tokio_util::sync::CancellationToken;
use tracing::Level;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
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

    // Spawn server in background
    let server_handle = tokio::spawn(odyssey_hub_server::run_server(sender, cancel_token.clone()));

    let status = tokio::select! {
        status = handle_server_status(receiver) => status,
        signal = tokio::signal::ctrl_c() => {
            cancel_token.cancel();
            signal.map(|()| false).map_err(|e| format!("Failed to listen for Ctrl+C: {e}"))
        }
    };
    cancel_token.cancel();
    let result = server_handle.await;
    match (status, result) {
        (Err(error), _) => {
            tracing::error!("{error}");
            ExitCode::FAILURE
        }
        (Ok(true), Ok(_)) | (Ok(false), Ok(Ok(()))) => ExitCode::SUCCESS,
        (_, Ok(Err(error))) => {
            tracing::error!("Server failed: {error}");
            ExitCode::FAILURE
        }
        (_, Err(error)) => {
            tracing::error!("Server task failed: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `true` means an existing instance was successfully notified instead of starting.
async fn handle_server_status(
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<Message>,
) -> Result<bool, String> {
    loop {
        match receiver.recv().await {
            Some(Message::ServerInit(Ok(_))) => {
                tracing::info!("Server started");
            }
            Some(Message::ServerInit(Err(e))) => {
                // Check if another instance is already running
                if e.kind() == std::io::ErrorKind::AddrInUse {
                    tracing::warn!(
                        "Another instance is already running. Notifying it to bring window to front..."
                    );

                    // Try to connect to the existing instance and call BringToFront
                    let mut client = odyssey_hub_client::client::Client::default();
                    client
                        .connect()
                        .await
                        .map_err(|e| format!("Failed to connect to existing instance: {e}"))?;
                    client
                        .bring_to_front()
                        .await
                        .map_err(|e| format!("Failed to notify existing instance: {e}"))?;
                    tracing::info!("Successfully notified existing instance");
                    return Ok(true);
                }
                return Err(format!("Server start error: {e}"));
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
    Ok(false)
}
