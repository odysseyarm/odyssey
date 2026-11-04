#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI usage and argument errors go to the terminal"
)]

use std::process::ExitCode;

use eros::Context as _;
use odyssey_hub_server::{self, Message};
use tokio_util::sync::CancellationToken;
use tracing::Level;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let mut simulation_requested = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--sim" => simulation_requested = true,
            "--help" | "-h" => {
                println!(
                    "Usage: odyssey-hub [--sim]\n\n--sim  Enable the simulator (requires the sim compile feature).\n       ODYSSEY_SIM=1 also enables it in sim builds."
                );
                return ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("Unknown argument: {argument}");
                return ExitCode::FAILURE;
            }
        }
    }
    #[cfg(not(feature = "sim"))]
    if simulation_requested {
        eprintln!("--sim requires a build with the sim feature");
        return ExitCode::FAILURE;
    }
    #[cfg(feature = "sim")]
    let simulation_enabled = simulation_requested || odyssey_hub_server::sim::enabled();
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

    let server_cancel = cancel_token.clone();
    let server_handle = tokio::spawn(async move {
        #[cfg(feature = "sim")]
        if simulation_enabled {
            return odyssey_hub_server::run_server_with_sim(sender, server_cancel).await;
        }
        odyssey_hub_server::run_server(sender, server_cancel).await
    });

    let status = tokio::select! {
        status = handle_server_status(receiver) => status,
        signal = tokio::signal::ctrl_c() => {
            cancel_token.cancel();
            signal.map(|()| false).context("listening for Ctrl+C").map_err(Into::into)
        }
    };
    cancel_token.cancel();
    let result = server_handle.await;
    match (status, result) {
        (Err(error), _) => {
            tracing::error!("{error:?}");
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

/// `Ok(true)` means another instance was already running and was told to come to
/// the front, so this one should exit without starting.
async fn handle_server_status(
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<Message>,
) -> eros::Result<bool> {
    loop {
        match receiver.recv().await {
            Some(Message::ServerInit(Ok(_))) => {
                tracing::info!("Server started");
            }
            Some(Message::ServerInit(Err(e))) => {
                // AddrInUse: another hub instance already owns the socket.
                if e.kind() == std::io::ErrorKind::AddrInUse {
                    tracing::warn!(
                        "Another instance is already running. Notifying it to bring window to front..."
                    );

                    let mut client = odyssey_hub_client::client::Client::default();
                    client
                        .connect()
                        .await
                        .context("connecting to the instance that owns the hub socket")?;
                    client
                        .bring_to_front()
                        .await
                        .context("asking the running instance to come to the front")?;
                    tracing::info!("Successfully notified existing instance");
                    return Ok(true);
                }
                return Err(e)
                    .context("starting the hub server")
                    .map_err(Into::into);
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
