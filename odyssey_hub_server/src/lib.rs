mod accessories;
mod device_link;
mod device_tasks;
mod server;
mod usb_hub_link;
mod usb_link;

use std::{pin::Pin, sync::Arc};

use futures::stream::{self, StreamExt};
use futures_concurrency::prelude::*;
use interprocess::local_socket::{
    traits::tokio::Listener, GenericFilePath, GenericNamespaced, ListenerOptions, NameType as _,
    ToFsName, ToNsName,
};
#[cfg(target_os = "windows")]
use interprocess::os::windows::{
    local_socket::ListenerOptionsExt,
    security_descriptor::{AsSecurityDescriptorMutExt, SecurityDescriptor},
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

/// Events from the run_server merged stream
#[derive(Debug)]
enum ServerEvent {
    DeviceTasks,
    AccessoryManager,
    EventForward,
    TonicServer,
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

    let (sender, receiver) = mpsc::channel(12);

    let (event_sender, _) = broadcast::channel(12);

    let screen_calibrations = odyssey_hub_common::config::screen_calibrations_async()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load screen calibrations: {}", e))?;
    let screen_calibrations = Arc::new(arc_swap::ArcSwap::from(Arc::new(screen_calibrations)));

    println!("Screen calibrations loaded: {:?}", screen_calibrations);

    let device_offsets = odyssey_hub_common::config::device_offsets_async()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load device offsets: {}", e))?;
    let device_offsets = Arc::new(tokio::sync::Mutex::new(device_offsets));

    let device_shot_delays = odyssey_hub_common::config::device_shot_delays_async()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load device shot delays: {}", e))?;

    let device_shot_delays = Arc::new(tokio::sync::RwLock::new(device_shot_delays));
    let shot_delay_watch = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    let accessory_info_map = odyssey_hub_common::config::accessory_map_async()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load accessory map: {}", e))?;

    let (accessory_info_sender, accessory_info_receiver) =
        tokio::sync::watch::channel(accessory_info_map.clone());

    let accessory_map = Arc::new(std::sync::Mutex::new(
        accessory_info_map
            .into_iter()
            .map(|(k, v)| (k, (v, false)))
            .collect(),
    ));

    let (accessory_map_sender, _) = broadcast::channel(12);

    let server = server::Server {
        device_list: Default::default(),
        event_sender: event_sender.clone(),
        screen_calibrations: screen_calibrations.clone(),
        device_offsets: device_offsets.clone(),
        device_shot_delays: device_shot_delays.clone(),
        shot_delay_watch: shot_delay_watch.clone(),
        accessory_map: accessory_map.clone(),
        accessory_map_sender: accessory_map_sender.clone(),
        accessory_info_sender,
    };

    let dl = server.device_list.clone();

    // Clone values before moving into streams
    let sender_clone = sender.clone();
    let screen_calibrations_clone = screen_calibrations.clone();
    let device_offsets_clone = device_offsets.clone();
    let device_shot_delays_clone = device_shot_delays.clone();
    let shot_delay_watch_clone = shot_delay_watch.clone();
    let accessory_map_clone = accessory_map.clone();
    let accessory_map_sender_clone1 = accessory_map_sender.clone();
    let accessory_map_sender_clone2 = accessory_map_sender.clone();
    let accessory_info_receiver_clone = accessory_info_receiver.clone();
    let event_sender_clone1 = event_sender.clone();
    let event_sender_clone2 = event_sender.clone();
    let event_sender_clone3 = event_sender.clone();
    let dl_clone1 = dl.clone();
    let dl_clone2 = dl.clone();

    // Convert device_tasks to stream
    let device_tasks_stream = stream::once(async move {
        tracing::info!("device_tasks_stream: starting");
        let result = device_tasks(
            sender_clone,
            screen_calibrations_clone,
            device_offsets_clone,
            device_shot_delays_clone,
            shot_delay_watch_clone,
            accessory_map_clone,
            accessory_map_sender_clone1,
            accessory_info_receiver_clone,
            event_sender_clone1,
        )
        .await;
        tracing::error!("device_tasks_stream: device_tasks returned {:?}", result);
        ServerEvent::DeviceTasks
    });

    // Convert accessory_manager to stream
    let accessory_stream = stream::once(async move {
        let _ = accessory_manager(
            event_sender_clone2,
            accessory_info_receiver,
            accessory_map_sender_clone2,
            dl_clone1,
        )
        .await;
        ServerEvent::AccessoryManager
    });

    // Convert event forwarding to stream
    let event_fwd_stream = stream::unfold(
        (receiver, dl_clone2, event_sender_clone3),
        |(mut recv, dl, event_sender)| async move {
            match recv.recv().await {
                Some(message) => {
                    match message {
                        device_tasks::Message::Connect(d1, sender) => {
                            dl.lock().push((d1.clone(), sender));
                        }
                        device_tasks::Message::Disconnect(d) => {
                            let mut dl = dl.lock();
                            let i = dl.iter().position(|a| (*a).0 == d);
                            if let Some(i) = i {
                                dl.remove(i);
                            }
                        }
                        device_tasks::Message::Event(e) => {
                            event_sender.send(e).unwrap();
                        }
                    }
                    Some((ServerEvent::EventForward, (recv, dl, event_sender)))
                }
                None => None,
            }
        },
    );

    // Convert tonic server to stream
    let tonic_stream = stream::once(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(ServiceServer::new(server))
            .serve_with_incoming_shutdown(listener, cancel_token.cancelled())
            .await;
        ServerEvent::TonicServer
    });

    // Merge all streams
    tracing::info!("run_server: merging streams");
    let merged = (
        device_tasks_stream,
        accessory_stream,
        event_fwd_stream,
        tonic_stream,
    )
        .merge();
    tokio::pin!(merged);
    tracing::info!("run_server: entering main loop");

    // Wait for any stream to complete (which means a critical task exited)
    while let Some(event) = merged.next().await {
        tracing::debug!("Server loop event: {:?}", event);
        match event {
            ServerEvent::DeviceTasks => {
                tracing::error!("Device tasks exited unexpectedly");
                break;
            }
            ServerEvent::AccessoryManager => {
                tracing::error!("Accessory manager exited unexpectedly");
                break;
            }
            ServerEvent::EventForward => {
                // Event forwarding continues, don't break
                tracing::trace!("Event forwarded");
            }
            ServerEvent::TonicServer => {
                tracing::info!("Tonic server exited");
                break;
            }
        }
    }

    tracing::info!("Server main loop ended");

    Ok(())
}
