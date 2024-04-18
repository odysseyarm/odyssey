use odyssey_hub_service::service::{self, Message};
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
    // tokio::spawn(handle_service_status(receiver));
    let cancel_token = CancellationToken::new();
    tokio::spawn({
        let cancel_token = cancel_token.clone();
        async move {
            tokio::signal::ctrl_c().await.expect("failed to listen for ctrl-c");
            cancel_token.cancel();
        }
    });
    service::run_service(sender, cancel_token).await.unwrap();
}

async fn handle_service_status(mut receiver: tokio::sync::mpsc::UnboundedReceiver<Message>) {
    loop {
        match receiver.recv().await {
            Some(Message::ServerInit(Ok(()))) => {
                eprintln!("Server started");
            }
            Some(Message::ServerInit(Err(_))) => {
                eprintln!("Server start error");
                break;
            }
            Some(Message::Stop) => {
                eprintln!("Server stopped");
                break;
            }
            None => {
                eprintln!("Server channel closed");
                break;
            }
        }
    }
}
