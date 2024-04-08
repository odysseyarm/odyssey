use odyssey_hub_service::service::{self, Message};
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
    service::run_service(sender).await.unwrap();
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
