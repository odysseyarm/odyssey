use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};
use interprocess::local_socket::{traits::tokio::Listener, ListenerOptions, NameTypeSupport, ToFsName, ToNsName};
use tokio::sync::mpsc;
use odyssey_hub_service_interface::{greeter_server::{Greeter, GreeterServer}, HelloReply, HelloRequest};
use tonic::transport::server::Connected;


#[derive(Debug, Default)]
struct Server {}

#[tonic::async_trait]
impl Greeter for Server {
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        println!("Got a request: {:?}", request);
        let reply = HelloReply {
            message: format!("Hello {}!", request.into_inner().name), // We must use .into_inner() as the fields of gRPC requests and responses are private
        };

        Ok(tonic::Response::new(reply)) // Send back our formatted greeting}
    }
}

pub enum Message {
    ServerInit(Result<(), std::io::Error>),
    Stop,
}

struct LocalSocketStream(interprocess::local_socket::tokio::Stream);

impl LocalSocketStream {
    fn inner_pin(self: Pin<&mut Self>) -> Pin<&mut interprocess::local_socket::tokio::Stream> {
        unsafe {
            self.map_unchecked_mut(|s| &mut s.0)
        }
    }
}

impl Connected for LocalSocketStream {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {
    }
}

impl AsyncWrite for LocalSocketStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.inner_pin().poll_write(cx, buf)
    }

    fn poll_flush(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), std::io::Error>> {
        self.inner_pin().poll_flush(cx)
    }

    fn poll_shutdown(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), std::io::Error>> {
        self.inner_pin().poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.inner_pin().poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }
}

impl AsyncRead for LocalSocketStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.inner_pin().poll_read(cx, buf)
    }
}

impl Drop for LocalSocketStream {
    fn drop(&mut self) {
        println!("connection closed");
    }
}

pub async fn run_service(sender: mpsc::UnboundedSender<Message>) -> anyhow::Result<()> {
    let name = {
        use NameTypeSupport as Nts;
        match NameTypeSupport::query() {
            Nts::OnlyFs => "/tmp/odyhub.sock".to_fs_name().unwrap(),
            Nts::OnlyNs | Nts::Both => "@odyhub.sock".to_ns_name().unwrap(),
        }
    };
    // Create our listener. In a more robust program, we'd check for an
    // existing socket file that has not been deleted for whatever reason,
    // ensure it's a socket file and not a normal file, and delete it.
    let listener = ListenerOptions::new().name(name.clone()).create_tokio().unwrap();
    let listener = futures::stream::unfold((), |()| {
        async {
            let conn = listener.accept().await;
            dbg!(&conn);
            Some((conn.map(LocalSocketStream), ()))
        }
    });
    println!("Server running at {:?}", name);

    sender.send(Message::ServerInit(Ok(()))).unwrap();

    dbg!(tonic::transport::Server::builder()
        .add_service(GreeterServer::new(Server::default()))
        .serve_with_incoming(listener)
        .await)?;

    Ok(())
}
