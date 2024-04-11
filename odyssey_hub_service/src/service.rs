use std::{mem::uninitialized, pin::Pin};

use tokio::io::{AsyncRead, AsyncWrite};
use interprocess::{local_socket::{traits::tokio::Listener, ListenerOptions, NameTypeSupport, ToFsName, ToNsName}, os::windows::{local_socket::ListenerOptionsExt, AsSecurityDescriptorMutExt, SecurityDescriptor}};
use tokio::sync::mpsc;
use odyssey_hub_service_interface::{service_server::{Service, ServiceServer}, DeviceListReply, DeviceListRequest, PollRequest, PollReply};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::server::Connected, Response};

use crate::device_tasks::{self, device_tasks};

#[derive(Debug)]
struct Server {
    device_list: std::sync::Arc<tokio::sync::Mutex<Vec<odyssey_hub_common::device::Device>>>,
    event_channel: std::sync::Arc<tokio::sync::Mutex<mpsc::Receiver<odyssey_hub_common::events::Event>>>,
}

#[tonic::async_trait]
impl Service for Server {
    type PollStream = ReceiverStream<Result<PollReply, tonic::Status>>;

    async fn get_device_list(
        &self,
        _: tonic::Request<DeviceListRequest>,
    ) -> Result<tonic::Response<DeviceListReply>, tonic::Status> {
        let device_list = self.device_list.lock().await.iter().map(|d| {
            d.clone().into()
        }).collect::<>();

        let reply = DeviceListReply {
            device_list,
        };

        Ok(tonic::Response::new(reply))
    }

    async fn poll(
        &self,
        _: tonic::Request<PollRequest>,
    ) -> tonic::Result<tonic::Response<Self::PollStream>, tonic::Status> {
        let (mut tx, rx) = mpsc::channel::<Result<PollReply, tonic::Status>>(12);
        tokio::spawn({
            let event_channel = self.event_channel.clone();
            async move {
                while let Some(event) = event_channel.lock().await.recv().await {
                    tx.send(Ok(PollReply { event: Some(event.into()) })).await.unwrap();
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

pub enum Message {
    ServerInit(Result<(), std::io::Error>),
    Stop,
}

/// Wrapper around interprocess's `LocalSocketStream` to implement the `Connected` trait and
/// override `poll_flush`.
#[derive(Debug)]
pub struct LocalSocketStream(interprocess::local_socket::tokio::Stream);

impl LocalSocketStream {
    pub fn new(inner: interprocess::local_socket::tokio::Stream) -> Self {
        Self(inner)
    }
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

    fn poll_flush(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
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
	let mut sd = SecurityDescriptor::new()?;
	unsafe {
        sd.set_dacl(std::ptr::null_mut(), false)?;
	}
    let listener = ListenerOptions::new().security_descriptor(sd).name(name.clone()).create_tokio().unwrap();
    let listener = futures::stream::unfold((), |()| {
        async {
            let conn = listener.accept().await;
            dbg!(&conn);
            Some((conn.map(LocalSocketStream), ()))
        }
    });

    sender.send(Message::ServerInit(Ok(()))).unwrap();
    println!("Server running at {:?}", name);

    let (sender, mut receiver) = mpsc::channel(12);

    let (event_sender, event_receiver) = mpsc::channel(12);

    let server = Server {
        device_list: Default::default(),
        event_channel: std::sync::Arc::new(tokio::sync::Mutex::new(event_receiver)),
    };

    let dl = server.device_list.clone();

    let handle = async {
        tokio::select! {
            _ = device_tasks(sender.clone()) => {},
            _ = async {
                while let Some(message) = receiver.recv().await {
                    match message {
                        device_tasks::Message::Connect(d) => {
                            dl.lock().await.push(d);
                        },
                        device_tasks::Message::Disconnect(d) => {
                            let mut dl = dl.lock().await;
                            let i = dl.iter().position(|a| *a == d);
                            if let Some(i) = i {
                                dl.remove(i);
                            }
                        },
                        device_tasks::Message::Event(e) => {
                            event_sender.send(e).await.unwrap();
                        },
                    }
                }
            } => {},
        };
    };

    dbg!(tokio::select! {
        _ = handle => {},
        _ = tonic::transport::Server::builder()
            .add_service(ServiceServer::new(server))
            .serve_with_incoming(listener) => {},
    });

    Ok(())
}
