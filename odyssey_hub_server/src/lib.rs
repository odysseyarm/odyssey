mod device_tasks;

use std::{pin::Pin, sync::Arc};

use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use ats_cv::ScreenCalibration;
use interprocess::local_socket::{
    traits::tokio::Listener, ListenerOptions, NameTypeSupport, ToFsName, ToNsName,
};
#[cfg(target_os = "windows")]
use interprocess::os::windows::{
    local_socket::ListenerOptionsExt, AsSecurityDescriptorMutExt, SecurityDescriptor,
};
use odyssey_hub_common::config;
use odyssey_hub_server_interface::{
    service_server::{Service, ServiceServer},
    DeviceListReply, DeviceListRequest, EmptyReply, GetShotDelayReply, PollReply, PollRequest,
    ResetShotDelayReply, SetShotDelayRequest, Vector2,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{broadcast, mpsc},
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::transport::server::Connected;

use crate::device_tasks::device_tasks;

#[derive(Debug)]
struct Server {
    device_list: Arc<
        parking_lot::Mutex<
            Vec<(
                odyssey_hub_common::device::Device,
                ats_usb::device::UsbDevice,
                tokio::sync::mpsc::Sender<device_tasks::DeviceTaskMessage>,
            )>,
        >,
    >,
    event_channel: broadcast::Sender<odyssey_hub_common::events::Event>,
    screen_calibrations: Arc<
        ArcSwap<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
    device_offsets:
        Arc<tokio::sync::Mutex<std::collections::HashMap<u64, nalgebra::Isometry3<f32>>>>,
    device_shot_delays: std::sync::Mutex<std::collections::HashMap<u64, u8>>,
}

#[tonic::async_trait]
impl Service for Server {
    type PollStream = ReceiverStream<Result<PollReply, tonic::Status>>;

    async fn get_device_list(
        &self,
        _: tonic::Request<DeviceListRequest>,
    ) -> Result<tonic::Response<DeviceListReply>, tonic::Status> {
        let device_list = self
            .device_list
            .lock()
            .iter()
            .map(|d| d.0.clone().into())
            .collect();

        let reply = DeviceListReply { device_list };

        Ok(tonic::Response::new(reply))
    }

    async fn poll(
        &self,
        _: tonic::Request<PollRequest>,
    ) -> tonic::Result<tonic::Response<Self::PollStream>, tonic::Status> {
        let (tx, rx) = mpsc::channel(12);
        let mut event_channel = self.event_channel.subscribe();
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
                let send_result = tx
                    .send(Ok(PollReply {
                        event: Some(event.into()),
                    }))
                    .await;
                if send_result.is_err() {
                    break;
                }
            }
        });
        Ok(tonic::Response::new(ReceiverStream::new(rx)))
    }

    async fn write_vendor(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::WriteVendorRequest>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::EmptyReply>, tonic::Status> {
        let odyssey_hub_server_interface::WriteVendorRequest { device, tag, data } =
            request.into_inner();

        let device = device.unwrap().into();

        let d: ats_usb::device::UsbDevice;

        if let Some(_device) = self.device_list.lock().iter().find(|d| d.0 == device) {
            d = _device.1.clone();
        } else {
            return Err(tonic::Status::not_found("Device not found in device list"));
        }

        match d.write_vendor(tag as u8, data.as_slice()).await {
            Ok(_) => {}
            Err(e) => {
                return Err(tonic::Status::aborted(format!(
                    "Failed to write vendor: {}",
                    e
                )));
            }
        }

        Ok(tonic::Response::new(
            odyssey_hub_server_interface::EmptyReply {},
        ))
    }

    async fn reset_zero(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::Device>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::EmptyReply>, tonic::Status> {
        let device = request.into_inner().into();

        let s;

        if let Some(_device) = self.device_list.lock().iter().find(|d| d.0 == device) {
            s = _device.2.clone();
        } else {
            return Err(tonic::Status::not_found("Device not found in device list"));
        }

        match s.send(device_tasks::DeviceTaskMessage::ResetZero).await {
            Ok(_) => {}
            Err(e) => {
                return Err(tonic::Status::aborted(format!(
                    "Failed to send reset zero device task message: {}",
                    e
                )));
            }
        }

        Ok(tonic::Response::new(
            odyssey_hub_server_interface::EmptyReply {},
        ))
    }

    async fn zero(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::ZeroRequest>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::EmptyReply>, tonic::Status> {
        let odyssey_hub_server_interface::ZeroRequest {
            device,
            translation,
            target,
        } = request.into_inner();

        let device = device.unwrap().into();
        let translation = translation.unwrap();
        let translation = nalgebra::Translation3::new(translation.x, translation.y, translation.z);
        let target = target.unwrap();
        let target = nalgebra::Point2::new(target.x, target.y);

        let s;

        if let Some(_device) = self.device_list.lock().iter().find(|d| d.0 == device) {
            s = _device.2.clone();
        } else {
            return Err(tonic::Status::not_found("Device not found in device list"));
        }

        match s
            .send(device_tasks::DeviceTaskMessage::Zero(translation, target))
            .await
        {
            Ok(_) => {}
            Err(e) => {
                return Err(tonic::Status::aborted(format!(
                    "Failed to send zero device task message: {}",
                    e
                )));
            }
        }

        Ok(tonic::Response::new(
            odyssey_hub_server_interface::EmptyReply {},
        ))
    }

    async fn save_zero(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::Device>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::EmptyReply>, tonic::Status> {
        let device = request.into_inner().into();

        let s;

        if let Some(_device) = self.device_list.lock().iter().find(|d| d.0 == device) {
            s = _device.2.clone();
        } else {
            return Err(tonic::Status::not_found("Device not found in device list"));
        }

        match s.send(device_tasks::DeviceTaskMessage::SaveZero).await {
            Ok(_) => {}
            Err(e) => {
                return Err(tonic::Status::aborted(format!(
                    "Failed to send save zero device task message: {}",
                    e
                )));
            }
        }

        Ok(tonic::Response::new(
            odyssey_hub_server_interface::EmptyReply {},
        ))
    }

    async fn clear_zero(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::Device>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::EmptyReply>, tonic::Status> {
        let device = request.into_inner().into();

        let s;

        if let Some(_device) = self.device_list.lock().iter().find(|d| d.0 == device) {
            s = _device.2.clone();
        } else {
            return Err(tonic::Status::not_found("Device not found in device list"));
        }

        match s.send(device_tasks::DeviceTaskMessage::ClearZero).await {
            Ok(_) => {}
            Err(e) => {
                return Err(tonic::Status::aborted(format!(
                    "Failed to send clear zero device task message: {}",
                    e
                )));
            }
        }

        Ok(tonic::Response::new(
            odyssey_hub_server_interface::EmptyReply {},
        ))
    }

    async fn get_screen_info_by_id(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::ScreenInfoByIdRequest>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::ScreenInfoReply>, tonic::Status> {
        let odyssey_hub_server_interface::ScreenInfoByIdRequest { id } = request.into_inner();

        let screen_calibrations = self.screen_calibrations.load();

        let screen_calibration = screen_calibrations
            .iter()
            .find(|(i, _)| *i as u32 == id)
            .map(|(_, calibration)| calibration.clone());

        let reply = odyssey_hub_server_interface::ScreenInfoReply {
            id,
            bounds: {
                if let Some(screen_calibration) = screen_calibration {
                    let bounds = screen_calibration.bounds();
                    Some(odyssey_hub_server_interface::ScreenBounds {
                        tl: Some({
                            Vector2 {
                                x: bounds[0].x,
                                y: bounds[0].y,
                            }
                        }),
                        tr: Some({
                            Vector2 {
                                x: bounds[1].x,
                                y: bounds[1].y,
                            }
                        }),
                        bl: Some({
                            Vector2 {
                                x: bounds[2].x,
                                y: bounds[2].y,
                            }
                        }),
                        br: Some({
                            Vector2 {
                                x: bounds[3].x,
                                y: bounds[3].y,
                            }
                        }),
                    })
                } else {
                    None
                }
            },
        };

        Ok(tonic::Response::new(reply))
    }

    async fn reset_shot_delay(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::Device>,
    ) -> Result<tonic::Response<ResetShotDelayReply>, tonic::Status> {
        let device: odyssey_hub_common::device::Device = request.into_inner().into();

        let defaults = tokio::task::spawn_blocking(|| {
            odyssey_hub_common::config::device_shot_delays()
                .map_err(|e| anyhow::anyhow!("config error: {}", e))
        })
        .await
        .map_err(|e| tonic::Status::internal(format!("reload join error: {}", e)))?
        .map_err(|e| tonic::Status::internal(e.to_string()))?;

        let delay_ms = defaults.get(&device.uuid()).cloned().unwrap_or(0);
        self.device_shot_delays
            .lock()
            .unwrap()
            .insert(device.uuid(), delay_ms);

        if let Err(e) = self
            .event_channel
            .send(odyssey_hub_common::events::Event::DeviceEvent(
                odyssey_hub_common::events::DeviceEvent(
                    device.clone(),
                    odyssey_hub_common::events::DeviceEventKind::ShotDelayChangedEvent(delay_ms),
                ),
            ))
        {
            return Err(tonic::Status::internal(format!(
                "Failed to send event: {}",
                e
            )));
        }

        Ok(tonic::Response::new(ResetShotDelayReply {
            delay_ms: delay_ms as u32,
        }))
    }

    async fn get_shot_delay(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::Device>,
    ) -> Result<tonic::Response<GetShotDelayReply>, tonic::Status> {
        let device: odyssey_hub_common::device::Device = request.into_inner().into();

        let delay_ms = self
            .device_shot_delays
            .lock()
            .unwrap()
            .get(&device.uuid())
            .cloned()
            .unwrap_or(0)
            .into();

        Ok(tonic::Response::new(GetShotDelayReply { delay_ms }))
    }

    async fn set_shot_delay(
        &self,
        request: tonic::Request<SetShotDelayRequest>,
    ) -> Result<tonic::Response<EmptyReply>, tonic::Status> {
        let SetShotDelayRequest { device, delay_ms } = request.into_inner();
        let device: odyssey_hub_common::device::Device = device.unwrap().into();
        let delay_ms = delay_ms as u8;

        self.device_shot_delays
            .lock()
            .unwrap()
            .insert(device.uuid(), delay_ms);

        if let Err(e) = self
            .event_channel
            .send(odyssey_hub_common::events::Event::DeviceEvent(
                odyssey_hub_common::events::DeviceEvent(
                    device.clone(),
                    odyssey_hub_common::events::DeviceEventKind::ShotDelayChangedEvent(delay_ms),
                ),
            ))
        {
            return Err(tonic::Status::internal(format!(
                "Failed to send event: {}",
                e
            )));
        }

        Ok(tonic::Response::new(EmptyReply {}))
    }

    async fn save_shot_delay(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::Device>,
    ) -> Result<tonic::Response<EmptyReply>, tonic::Status> {
        let device: odyssey_hub_common::device::Device = request.into_inner().into();
        let uuid = device.uuid();

        let delay: u8 = {
            let guard = self.device_shot_delays.lock().unwrap();
            guard
                .get(&uuid)
                .cloned()
                .ok_or_else(|| tonic::Status::not_found("No shot-delay entry for this device"))?
        };

        tokio::task::spawn_blocking(move || {
            config::save_device_shot_delay(uuid, delay)
                .map_err(|e| anyhow::anyhow!("failed to save shot-delay: {}", e))
        })
        .await
        .map_err(|e| tonic::Status::internal(format!("blocking join error: {}", e)))?
        .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(EmptyReply {}))
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

    let server = Server {
        device_list: Default::default(),
        event_channel: event_sender.clone(),
        screen_calibrations: screen_calibrations.clone(),
        device_offsets: device_offsets.clone(),
        device_shot_delays: device_shot_delays.into(),
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
        _ = event_fwd => {},
        _ = tonic::transport::Server::builder()
            .add_service(ServiceServer::new(server))
            .serve_with_incoming_shutdown(listener, cancel_token.cancelled()) => {},
    }

    Ok(())
}
