use std::{fs::File, pin::Pin, sync::Arc};

use app_dirs2::{get_app_root, AppDataType, AppInfo};
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
use odyssey_hub_service_interface::{
    service_server::{Service, ServiceServer},
    DeviceListReply, DeviceListRequest, PollReply, PollRequest, Vector2,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{broadcast, mpsc},
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{transport::server::Connected, Response};
use tracing::error;

use crate::device_tasks::{self, device_tasks};

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
    event_channel: broadcast::Receiver<odyssey_hub_common::events::Event>,
    screen_calibrations: Arc<
        ArcSwap<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
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
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn write_vendor(
        &self,
        request: tonic::Request<odyssey_hub_service_interface::WriteVendorRequest>,
    ) -> Result<tonic::Response<odyssey_hub_service_interface::WriteVendorReply>, tonic::Status>
    {
        let odyssey_hub_service_interface::WriteVendorRequest { device, tag, data } =
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

        Ok(Response::new(
            odyssey_hub_service_interface::WriteVendorReply {},
        ))
    }

    async fn reset_zero(
        &self,
        request: tonic::Request<odyssey_hub_service_interface::Device>,
    ) -> Result<tonic::Response<odyssey_hub_service_interface::ResetZeroReply>, tonic::Status> {
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

        Ok(Response::new(
            odyssey_hub_service_interface::ResetZeroReply {},
        ))
    }

    async fn zero(
        &self,
        request: tonic::Request<odyssey_hub_service_interface::ZeroRequest>,
    ) -> Result<tonic::Response<odyssey_hub_service_interface::ZeroReply>, tonic::Status> {
        let odyssey_hub_service_interface::ZeroRequest {
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

        Ok(Response::new(odyssey_hub_service_interface::ZeroReply {}))
    }

    async fn get_screen_info_by_id(
        &self,
        request: tonic::Request<odyssey_hub_service_interface::ScreenInfoByIdRequest>,
    ) -> Result<tonic::Response<odyssey_hub_service_interface::ScreenInfoResponse>, tonic::Status>
    {
        let odyssey_hub_service_interface::ScreenInfoByIdRequest { id } =
            request.into_inner();

        let screen_calibrations = self.screen_calibrations.load();

        let screen_calibration = screen_calibrations
            .iter()
            .find(|(i, _)| *i as u32 == id)
            .map(|(_, calibration)| calibration.clone());

        let reply = odyssey_hub_service_interface::ScreenInfoResponse {
            id,
            bounds: {
                if let Some(screen_calibration) = screen_calibration {
                    let bounds = screen_calibration.bounds();
                    Some(odyssey_hub_service_interface::ScreenBounds {
                        tl: Some({
                            Vector2 { x: bounds[0].x, y: bounds[0].y }
                        }),
                        tr: Some({
                            Vector2 { x: bounds[1].x, y: bounds[1].y }
                        }),
                        bl: Some({
                            Vector2 { x: bounds[2].x, y: bounds[2].y }
                        }),
                        br: Some({
                            Vector2 { x: bounds[3].x, y: bounds[3].y }
                        }),
                    })
                } else {
                    None
                }
            }
        };

        Ok(Response::new(reply))
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

pub async fn run_service(
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

    let (event_sender, event_receiver) = broadcast::channel(12);

    pub const APP_INFO: AppInfo = AppInfo {
        name: "odyssey",
        author: "odysseyarm",
    };

    let screen_calibrations: arrayvec::ArrayVec<
        (u8, ScreenCalibration<f32>),
        { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
    > = (0..{ (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize })
        .filter_map(|i| {
            get_app_root(AppDataType::UserConfig, &APP_INFO)
                .ok()
                .and_then(|config_dir| {
                    let screen_path = config_dir
                        .join("screens")
                        .join(std::format!("screen_{}.json", i));
                    if screen_path.exists() {
                        File::open(screen_path)
                            .ok()
                            .and_then(|file| match serde_json::from_reader(file) {
                                Ok(calibration) => Some((i as u8, calibration)),
                                Err(e) => {
                                    error!("Failed to deserialize screen calibration: {}", e);
                                    None
                                }
                            })
                    } else {
                        None
                    }
                })
        })
        .collect();

    let screen_calibrations = Arc::new(arc_swap::ArcSwap::from(Arc::new(screen_calibrations)));
    let server = Server {
        device_list: Default::default(),
        event_channel: event_receiver,
        screen_calibrations: screen_calibrations.clone(),
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
                            odyssey_hub_common::events::DeviceEvent {
                                device: d1.clone(),
                                kind: odyssey_hub_common::events::DeviceEventKind::ConnectEvent,
                            }
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
                            odyssey_hub_common::events::DeviceEvent {
                                device: d.clone(),
                                kind: odyssey_hub_common::events::DeviceEventKind::DisconnectEvent,
                            }
                        }))
                        .unwrap();
                }
                device_tasks::Message::Event(e) => {
                    event_sender.send(e).unwrap();
                }
            }
        }
    };

    // let device_offsets: HashMap<[u8; 6], nalgebra::Isometry3<f32>> = get_app_root(AppDataType::UserConfig, &APP_INFO)
    //     .ok()
    //     .and_then(|config_dir| {
    //         let device_offsets_path = config_dir.join("device_offsets.json");
    //         if device_offsets_path.exists() {
    //             File::open(device_offsets_path)
    //                 .ok()
    //                 .and_then(|file| match serde_json::from_reader(file) {
    //                     Ok(offsets) => Some(offsets),
    //                     Err(e) => {
    //                         error!("Failed to deserialize device offsets: {}", e);
    //                         None
    //                     }
    //                 })
    //         } else {
    //             None
    //         }
    //     })
    //     .unwrap_or_default();

    // let device_offsets = Arc::new(tokio::sync::Mutex::new(device_offsets));

    tokio::select! {
        _ = device_tasks(sender, screen_calibrations.clone()) => {},
        _ = event_fwd => {},
        _ = tonic::transport::Server::builder()
            .add_service(ServiceServer::new(server))
            .serve_with_incoming_shutdown(listener, cancel_token.cancelled()) => {},
    }

    Ok(())
}
