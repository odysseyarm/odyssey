use std::{fs::File, pin::Pin, sync::Arc};

use app_dirs2::{get_app_root, AppDataType, AppInfo};
use ats_cv::ScreenCalibration;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{broadcast, mpsc},
};
use interprocess::local_socket::{traits::tokio::Listener, ListenerOptions, NameTypeSupport, ToFsName, ToNsName};
#[cfg(os = "windows")]
use os::windows::{local_socket::ListenerOptionsExt, AsSecurityDescriptorMutExt, SecurityDescriptor};
use odyssey_hub_service_interface::{service_server::{Service, ServiceServer}, DeviceListReply, DeviceListRequest, PollRequest, PollReply};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{transport::server::Connected, Response};
use tracing::{error, Level};
use tracing_subscriber::EnvFilter;

use crate::device_tasks::{self, device_tasks};

#[derive(Debug)]
struct Server {
    device_list: Arc<parking_lot::Mutex<Vec<odyssey_hub_common::device::Device>>>,
    event_channel: broadcast::Receiver<odyssey_hub_common::events::Event>,
}

#[tonic::async_trait]
impl Service for Server {
    type PollStream = ReceiverStream<Result<PollReply, tonic::Status>>;

    async fn get_device_list(
        &self,
        _: tonic::Request<DeviceListRequest>,
    ) -> Result<tonic::Response<DeviceListReply>, tonic::Status> {
        let device_list = self.device_list.lock().iter().map(|d| {
            d.clone().into()
        }).collect();

        let reply = DeviceListReply {
            device_list,
        };

        Ok(tonic::Response::new(reply))
    }

    async fn poll(
        &self,
        _: tonic::Request<PollRequest>,
    ) -> tonic::Result<tonic::Response<Self::PollStream>, tonic::Status> {
        let (tx, rx) = mpsc::channel(12);
        let mut event_channel = self.event_channel.resubscribe();
        tokio::spawn(async move {
            loop {
                let event = match event_channel.recv().await {
                    Ok(v) => v,
                    Err(broadcast::error::RecvError::Lagged(dropped)) => {
                        tracing::warn!("Dropping {dropped} events because the client is too slow");
                        event_channel = event_channel.resubscribe();
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let send_result = tx.send(Ok(PollReply { event: Some(event.into()) })).await;
                if send_result.is_err() {
                    break;
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

pub async fn run_service(sender: mpsc::UnboundedSender<Message>, cancel_token: CancellationToken) -> anyhow::Result<()> {
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
    let listener_opts = ListenerOptions::new().name(name.clone());
    #[cfg(os = "windows")]
    let listener_opts = {
        let mut sd = SecurityDescriptor::new()?;
        unsafe {
            sd.set_dacl(std::ptr::null_mut(), false)?;
        }
        listener_opts.security_descriptor(sd);
    };
    let listener = listener_opts.create_tokio().unwrap();
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

    let (event_sender, event_receiver) = broadcast::channel(12);

    let server = Server {
        device_list: Default::default(),
        event_channel: event_receiver,
    };

    let dl = server.device_list.clone();

    let event_fwd = async {
        // Forward events from the device tasks to subscribed clients
        while let Some(message) = receiver.recv().await {
            match message {
                device_tasks::Message::Connect(d) => {
                    dl.lock().push(d.clone());
                    event_sender.send(
                        odyssey_hub_common::events::Event::DeviceEvent({
                            odyssey_hub_common::events::DeviceEvent {
                                device: d.clone(),
                                kind: odyssey_hub_common::events::DeviceEventKind::ConnectEvent,
                            }
                        })
                    ).unwrap();
                },
                device_tasks::Message::Disconnect(d) => {
                    let mut dl = dl.lock();
                    let i = dl.iter().position(|a| *a == d);
                    if let Some(i) = i {
                        dl.remove(i);
                    }
                    event_sender.send(
                        odyssey_hub_common::events::Event::DeviceEvent({
                            odyssey_hub_common::events::DeviceEvent {
                                device: d.clone(),
                                kind: odyssey_hub_common::events::DeviceEventKind::DisconnectEvent,
                            }
                        })
                    ).unwrap();
                },
                device_tasks::Message::Event(e) => {
                    event_sender.send(e).unwrap();
                },
            }
        }
    };

    pub const APP_INFO: AppInfo = AppInfo{name: "odyssey", author: "odysseyarm"};

    let screen_calibrations: arrayvec::ArrayVec<(u8, ScreenCalibration<f64>), { (ats_cv::foveated::MAX_SCREEN_ID+1) as usize }> = (0..{(ats_cv::foveated::MAX_SCREEN_ID+1) as usize}).filter_map(|i| {
        get_app_root(AppDataType::UserConfig, &APP_INFO)
        .ok()
        .and_then(|config_dir| {
            let screen_path = config_dir.join("screens").join(std::format!("screen_{}.json", i));
            if screen_path.exists() {
                File::open(screen_path).ok().and_then(|file| {
                    match serde_json::from_reader(file) {
                        Ok(calibration) => Some((i as u8, calibration)),
                        Err(e) => {
                            error!("Failed to deserialize screen calibration: {}", e);
                            None
                        }
                    }
                })
            } else {
                None
            }
        })
    }).collect();

    let screen_calibrations = Arc::new(tokio::sync::Mutex::new(screen_calibrations));

    tokio::select! {
        _ = device_tasks(sender, screen_calibrations.clone()) => {},
        _ = event_fwd => {},
        _ = tonic::transport::Server::builder()
            .add_service(ServiceServer::new(server))
            .serve_with_incoming_shutdown(listener, cancel_token.cancelled()) => {},
    }

    Ok(())
}
