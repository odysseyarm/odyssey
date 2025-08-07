mod accessories;
mod device_tasks;
mod server;

use std::{pin::Pin, sync::Arc};

use interprocess::local_socket::{
    traits::tokio::Listener, GenericFilePath, GenericNamespaced, ListenerOptions, NameType as _, ToFsName, ToNsName
};
#[cfg(target_os = "windows")]
use interprocess::os::windows::{
    local_socket::ListenerOptionsExt,
    security_descriptor::{
        AsSecurityDescriptorMutExt,
        SecurityDescriptor,
    }
};
use odyssey_hub_server_interface::service_server::ServiceServer;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{broadcast, mpsc},
};
use tokio_util::sync::CancellationToken;
use tonic::transport::server::Connected;

use crate::{accessories::accessory_manager, device_tasks::device_tasks};

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
        unsafe { self.map_unchecked_mut(|s| &mut s.0) }
    }
}

impl Connected for LocalSocketStream {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {}
}

impl AsyncWrite for LocalSocketStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.inner_pin().poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
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

pub async fn run_server(
    sender: mpsc::UnboundedSender<Message>,
    cancel_token: CancellationToken,
) -> anyhow::Result<()> {
    let name = if GenericNamespaced::is_supported() {
        "@odyhub.sock".to_ns_name::<GenericNamespaced>()?
    } else {
        "/tmp/odyhub.sock".to_fs_name::<GenericFilePath>()?
    };
    // Create our listener. In a more robust program, we'd check for an
    // existing socket file that has not been deleted for whatever reason,
    // ensure it's a socket file and not a normal file, and delete it.
    let listener_opts = ListenerOptions::new().name(name.clone());
    #[cfg(target_os = "windows")]
    let listener_opts = {
        let mut sd = SecurityDescriptor::new()?;
        unsafe {
            sd.set_dacl(std::ptr::null_mut(), false)?;
        }
        listener_opts.security_descriptor(sd)
    };
    let listener = listener_opts.create_tokio().unwrap();
    let listener = futures::stream::unfold((), |()| async {
        let conn = listener.accept().await;
        dbg!(&conn);
        Some((conn.map(LocalSocketStream), ()))
    });

    sender.send(Message::ServerInit(Ok(()))).unwrap();
    println!("Server running at {:?}", name);

    let (sender, mut receiver) = mpsc::channel(12);

    let (event_sender, _) = broadcast::channel(12);

    let screen_calibrations = tokio::task::spawn_blocking(|| {
        odyssey_hub_common::config::screen_calibrations()
            .map_err(|e| anyhow::anyhow!("Failed to load screen calibrations: {}", e))
    })
    .await??;
    let screen_calibrations = Arc::new(arc_swap::ArcSwap::from(Arc::new(screen_calibrations)));

    println!("Screen calibrations loaded: {:?}", screen_calibrations);

    let device_offsets = tokio::task::spawn_blocking(|| {
        odyssey_hub_common::config::device_offsets()
            .map_err(|e| anyhow::anyhow!("Failed to load device offsets: {}", e))
    })
    .await??;
    let device_offsets = Arc::new(tokio::sync::Mutex::new(device_offsets));

    let device_shot_delays = tokio::task::spawn_blocking(|| {
        odyssey_hub_common::config::device_shot_delays()
            .map_err(|e| anyhow::anyhow!("Failed to load device offsets: {}", e))
    })
    .await??;
    
    let accessory_info_map = tokio::task::spawn_blocking(|| {
        odyssey_hub_common::config::accessory_map()
            .map_err(|e| anyhow::anyhow!("Failed to load accessory map: {}", e))
    })
    .await??;

    let (accessory_info_sender, accessory_info_receiver) = tokio::sync::watch::channel(accessory_info_map.clone());
    
    let accessory_map = Arc::new(std::sync::Mutex::new(
        accessory_info_map.into_iter().map(|(k, v)| (k, (v, false))).collect()
    ));

    let (accessory_map_sender, _) = broadcast::channel(12);

    let server = server::Server {
        device_list: Default::default(),
        event_sender: event_sender.clone(),
        screen_calibrations: screen_calibrations.clone(),
        device_offsets: device_offsets.clone(),
        device_shot_delays: device_shot_delays.into(),
        accessory_map: accessory_map.clone(),
        accessory_map_sender: accessory_map_sender.clone(),
        accessory_info_sender,
    };

    let dl = server.device_list.clone();

    let event_fwd = async {
        // Forward events from the device tasks to subscribed clients
        while let Some(message) = receiver.recv().await {
            match message {
                device_tasks::Message::Connect(d1, d2, sender) => {
                    dl.lock().push((d1.clone(), d2.clone(), sender));
                    event_sender
                        .send(odyssey_hub_common::events::Event::DeviceEvent({
                            odyssey_hub_common::events::DeviceEvent(
                                d1.clone(),
                                odyssey_hub_common::events::DeviceEventKind::ConnectEvent,
                            )
                        }))
                        .unwrap();
                }
                device_tasks::Message::Disconnect(d) => {
                    let mut dl = dl.lock();
                    let i = dl.iter().position(|a| (*a).0 == d);
                    if let Some(i) = i {
                        dl.remove(i);
                    }
                    event_sender
                        .send(odyssey_hub_common::events::Event::DeviceEvent({
                            odyssey_hub_common::events::DeviceEvent(
                                d.clone(),
                                odyssey_hub_common::events::DeviceEventKind::DisconnectEvent,
                            )
                        }))
                        .unwrap();
                }
                device_tasks::Message::Event(e) => {
                    event_sender.send(e).unwrap();
                }
            }
        }
    };
    
    tokio::select! {
        _ = device_tasks(sender, screen_calibrations.clone(), device_offsets.clone()) => {},
        _ = accessory_manager(accessory_info_receiver, accessory_map_sender.clone()) => {},
        _ = event_fwd => {},
        _ = tonic::transport::Server::builder()
            .add_service(ServiceServer::new(server))
            .serve_with_incoming_shutdown(listener, cancel_token.cancelled()) => {},
    }

    Ok(())
}
