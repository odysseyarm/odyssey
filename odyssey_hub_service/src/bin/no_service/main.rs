#[path = "../service/service.rs"] mod service;

use service::Message;

#[tokio::main]
async fn main() {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    tokio::select! {
        _ = tokio::spawn(service::run_service(sender)) => {},
        _ = tokio::spawn(handle_service_status(receiver)) => {},
    };
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
