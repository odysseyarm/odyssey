use futures::StreamExt as _;
use interprocess::local_socket::tokio::prelude::LocalSocketStream;
use interprocess::local_socket::traits::tokio::Stream as _;
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, NameType as _, ToFsName as _, ToNsName as _,
};
use odyssey_hub_common as common;
use odyssey_hub_server_interface as proto;
use odyssey_hub_server_interface::service_client::ServiceClient;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

pub use crate::dongle::DongleInfo;
pub use crate::error::{ClientError, RpcError};

/// Connection to the hub's local gRPC socket.
///
/// Clones share one connection and may be used concurrently. Device-specific
/// calls go through [`Client::device`].
#[derive(Clone, Debug)]
pub struct Client {
    rpc: ServiceClient<Channel>,
}

/// Calls addressed to one device, holding that device's wire form.
///
/// Clones share the [`Client`]'s connection.
#[derive(Clone, Debug)]
pub struct DeviceClient {
    rpc: ServiceClient<Channel>,
    device: proto::Device,
}

fn invalid_reply(message: &'static str) -> RpcError {
    tonic::Status::data_loss(message).into()
}

fn devices_from_reply(
    reply: proto::DeviceListReply,
) -> Result<Vec<common::device::Device>, RpcError> {
    reply
        .device_list
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<_, _>>()
        .map_err(invalid_reply)
}

fn event_stream(
    stream: impl futures::Stream<Item = Result<proto::Event, tonic::Status>>,
) -> impl futures::Stream<Item = Result<common::events::Event, RpcError>> + Unpin {
    // Malformed events do not terminate the subscription.
    Box::pin(stream.filter_map(|item| async move {
        match item {
            Ok(event) => match common::events::Event::try_from(event) {
                Ok(event) => Some(Ok(event)),
                Err(()) => {
                    tracing::warn!("Ignoring malformed or unsupported hub event");
                    None
                }
            },
            Err(status) => Some(Err(RpcError::from(status))),
        }
    }))
}

/// Image frames whose device lease is renewed from `poll_next`, so renewals stop
/// when the consumer stops polling, not merely when the process exits.
struct LeasedImages<S> {
    frames: S,
    renewals: futures::channel::mpsc::Sender<proto::StreamImagesRequest>,
    interval: tokio::time::Interval,
}

impl<S: futures::Stream + Unpin> futures::Stream for LeasedImages<S> {
    type Item = S::Item;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<S::Item>> {
        while self.interval.poll_tick(cx).is_ready() {
            // Full means a renewal is already queued.
            let _ = self
                .renewals
                .try_send(proto::StreamImagesRequest::default());
        }
        self.frames.poll_next_unpin(cx)
    }
}

impl Client {
    /// Connects to the hub on this machine.
    ///
    /// The connection is driven by a task on the Tokio runtime that is current
    /// when this is called; once that runtime shuts down, every call on this
    /// client and its clones fails. Dropping a returned stream ends that
    /// subscription on the hub.
    ///
    /// # Errors
    ///
    /// [`ClientError::Io`] if the socket name is invalid on this platform,
    /// [`ClientError::Transport`] if no hub accepts the connection.
    pub async fn connect() -> Result<Self, ClientError> {
        let name = if GenericNamespaced::is_supported() {
            "@odyhub.sock".to_ns_name::<GenericNamespaced>()?
        } else {
            "/tmp/odyhub.sock".to_fs_name::<GenericFilePath>()?
        };

        // Connect eagerly rather than lazily: no RPC works without the socket. The
        // connector dials the local socket and ignores this URI.
        let channel = Endpoint::try_from("http://[::]:50051")?
            .connect_with_connector(service_fn(move |_: Uri| {
                let name = name.clone();
                async move {
                    let r = LocalSocketStream::connect(name).await?;
                    std::io::Result::Ok(r)
                }
            }))
            .await?;

        Ok(Self {
            rpc: ServiceClient::new(channel),
        })
    }

    pub fn device(&self, device: common::device::Device) -> DeviceClient {
        DeviceClient {
            rpc: self.rpc.clone(),
            device: device.into(),
        }
    }

    pub async fn get_device_list(&self) -> Result<Vec<common::device::Device>, ClientError> {
        let reply = self
            .rpc
            .clone()
            .get_device_list(proto::DeviceListRequest {})
            .await?
            .into_inner();
        Ok(devices_from_reply(reply)?)
    }

    pub async fn bring_to_front(&self) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .bring_to_front(proto::EmptyRequest {})
            .await?;
        Ok(())
    }

    pub async fn subscribe_device_list(
        &self,
    ) -> Result<
        impl futures::Stream<Item = Result<Vec<common::device::Device>, RpcError>> + use<>,
        ClientError,
    > {
        let stream = self
            .rpc
            .clone()
            .subscribe_device_list(proto::SubscribeDeviceListRequest {})
            .await?
            .into_inner();

        Ok(stream.map(|item| item.map_err(RpcError::from).and_then(devices_from_reply)))
    }

    pub async fn get_accessory_map(&self) -> Result<common::accessory::AccessoryMap, ClientError> {
        let reply = self
            .rpc
            .clone()
            .get_accessory_map(proto::AccessoryMapRequest {})
            .await?
            .into_inner();
        Ok(reply.try_into().map_err(invalid_reply)?)
    }

    pub async fn subscribe_accessory_map(
        &self,
    ) -> Result<
        impl futures::Stream<Item = Result<common::accessory::AccessoryMap, RpcError>> + use<>,
        ClientError,
    > {
        let stream = self
            .rpc
            .clone()
            .subscribe_accessory_map(proto::SubscribeAccessoryMapRequest {})
            .await?
            .into_inner();

        Ok(stream.map(|item| {
            item.map_err(RpcError::from)
                .and_then(|reply| reply.try_into().map_err(invalid_reply))
        }))
    }

    /// Streams hub events until the peer closes or the stream is dropped.
    pub async fn subscribe_events(
        &self,
    ) -> Result<
        impl futures::Stream<Item = Result<common::events::Event, RpcError>> + Unpin + use<>,
        ClientError,
    > {
        let stream = self
            .rpc
            .clone()
            .subscribe_events(proto::SubscribeEventsRequest {})
            .await?
            .into_inner();

        Ok(event_stream(stream))
    }

    pub async fn get_screen_info_by_id(&self, id: u8) -> Result<common::ScreenInfo, ClientError> {
        let reply = self
            .rpc
            .clone()
            .get_screen_info_by_id(proto::ScreenInfoByIdRequest { id: id.into() })
            .await?
            .into_inner();
        Ok(reply.try_into().map_err(invalid_reply)?)
    }

    pub async fn update_accessory_info_map(
        &self,
        map: common::accessory::AccessoryInfoMap,
    ) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .update_accessory_info_map(proto::AccessoryInfoMap::from(map))
            .await?;
        Ok(())
    }

    pub async fn subscribe_dongle_list(
        &self,
    ) -> Result<impl futures::Stream<Item = Result<Vec<DongleInfo>, RpcError>> + use<>, ClientError>
    {
        let stream = self
            .rpc
            .clone()
            .subscribe_dongle_list(proto::SubscribeDongleListRequest {})
            .await?
            .into_inner();

        Ok(stream.map(|item| {
            item.map_err(RpcError::from).and_then(|reply| {
                reply
                    .dongle_list
                    .into_iter()
                    .map(DongleInfo::try_from)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(invalid_reply)
            })
        }))
    }

    pub async fn start_dongle_pairing(
        &self,
        dongle_id: String,
        timeout_ms: u32,
    ) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .start_dongle_pairing(proto::StartDonglePairingRequest {
                dongle_id,
                timeout_ms,
            })
            .await?;
        Ok(())
    }

    pub async fn cancel_dongle_pairing(&self, dongle_id: String) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .cancel_dongle_pairing(proto::CancelDonglePairingRequest { dongle_id })
            .await?;
        Ok(())
    }

    pub async fn clear_dongle_bonds(&self, dongle_id: String) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .clear_dongle_bonds(proto::ClearDongleBondsRequest { dongle_id })
            .await?;
        Ok(())
    }
}

impl DeviceClient {
    /// Stream one USB camera in native image coordinates.
    ///
    /// Acquisition and backpressure can lower the requested FPS. Image-only firmware
    /// supplies no object samples and pauses optical tracking. Only one consumer may
    /// own a physical unit; dropping this stream ends capture.
    /// Polling the stream renews the device's capture lease; a consumer that stops
    /// polling for [`IMAGE_LEASE_TIMEOUT_MS`](odyssey_protocol::image::IMAGE_LEASE_TIMEOUT_MS)
    /// loses the session. Modern devices then restore tracking on their own; legacy
    /// devices require reboot/reconnect.
    /// Frames never enter the general event stream.
    pub async fn stream_images(
        &self,
        config: odyssey_protocol::image::ImageStreamConfig,
    ) -> Result<
        impl futures::Stream<Item = Result<odyssey_protocol::image::ImageFrame, RpcError>>
        + Unpin
        + Send
        + use<>,
        ClientError,
    > {
        let (mut renewals, requests) = futures::channel::mpsc::channel(1);
        renewals
            .try_send(proto::StreamImagesRequest {
                device: Some(self.device.clone()),
                camera: match config.camera {
                    odyssey_protocol::Port::Nf => 0,
                    odyssey_protocol::Port::Wf => 1,
                },
                fps: config.fps.into(),
            })
            .expect("a new channel has room for its first message");
        let stream = self.rpc.clone().stream_images(requests).await?.into_inner();
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(
            odyssey_protocol::image::IMAGE_LEASE_RENEW_INTERVAL_MS,
        ));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Ok(LeasedImages {
            frames: stream.map(|item| {
                item.map_err(RpcError::from)
                    .and_then(|frame| frame.try_into().map_err(invalid_reply))
            }),
            renewals,
            interval,
        })
    }

    /// Request reset over the current USB control endpoint. An acknowledgement
    /// does not confirm recovery: wait for a new connection and tracking reports.
    pub async fn reboot_device(&self) -> Result<(), ClientError> {
        self.rpc.clone().reboot_device(self.device.clone()).await?;
        Ok(())
    }

    pub async fn write_vendor(&self, tag: u8, data: Vec<u8>) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .write_vendor(proto::WriteVendorRequest {
                device: Some(self.device.clone()),
                tag: tag.into(),
                data,
            })
            .await?;
        Ok(())
    }

    pub async fn read_register(&self, port: u8, bank: u8, address: u8) -> Result<u8, ClientError> {
        let reply = self
            .rpc
            .clone()
            .read_register(proto::ReadRegisterRequest {
                device: Some(self.device.clone()),
                port: u32::from(port),
                bank: u32::from(bank),
                address: u32::from(address),
            })
            .await?
            .into_inner();
        Ok(u8::try_from(reply.data)
            .map_err(|_conversion_error| invalid_reply("register reply exceeds u8"))?)
    }

    pub async fn write_register(
        &self,
        port: u8,
        bank: u8,
        address: u8,
        data: u8,
    ) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .write_register(proto::WriteRegisterRequest {
                device: Some(self.device.clone()),
                port: u32::from(port),
                bank: u32::from(bank),
                address: u32::from(address),
                data: u32::from(data),
            })
            .await?;
        Ok(())
    }

    pub async fn read_config(&self, kind: u8) -> Result<u8, ClientError> {
        let reply = self
            .rpc
            .clone()
            .read_config(proto::ReadConfigRequest {
                device: Some(self.device.clone()),
                kind: u32::from(kind),
            })
            .await?
            .into_inner();
        Ok(u8::try_from(reply.value)
            .map_err(|_conversion_error| invalid_reply("config reply exceeds u8"))?)
    }

    pub async fn write_config(&self, kind: u8, value: u8) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .write_config(proto::WriteConfigRequest {
                device: Some(self.device.clone()),
                kind: u32::from(kind),
                value: u32::from(value),
            })
            .await?;
        Ok(())
    }

    pub async fn start_pairing(&self, timeout_ms: u32) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .start_pairing(proto::StartPairingRequest {
                device: Some(self.device.clone()),
                timeout_ms,
            })
            .await?;
        Ok(())
    }

    pub async fn cancel_pairing(&self) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .cancel_pairing(proto::CancelPairingRequest {
                device: Some(self.device.clone()),
            })
            .await?;
        Ok(())
    }

    pub async fn clear_bond(&self) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .clear_bond(proto::ClearBondRequest {
                device: Some(self.device.clone()),
            })
            .await?;
        Ok(())
    }

    /// Ask a device without [`DeviceCapabilities::AUTO_TRANSPORT`] to stream over USB
    /// (`usb_mode`) or Bluetooth. It stores the choice and reboots; the reply is the mode
    /// it confirmed. A device with `AUTO_TRANSPORT` picks its link itself, so the request
    /// is ignored and the reply says whether USB carries its events now.
    ///
    /// [`DeviceCapabilities::AUTO_TRANSPORT`]: common::device::DeviceCapabilities::AUTO_TRANSPORT
    pub async fn set_transport_mode(&self, usb_mode: bool) -> Result<bool, ClientError> {
        let reply = self
            .rpc
            .clone()
            .set_transport_mode(proto::SetTransportModeRequest {
                device: Some(self.device.clone()),
                usb_mode,
            })
            .await?
            .into_inner();
        Ok(reply.usb_mode)
    }

    pub async fn flash_settings(&self) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .flash_settings(proto::FlashSettingsRequest {
                device: Some(self.device.clone()),
            })
            .await?;
        Ok(())
    }

    pub async fn reset_zero(&self) -> Result<(), ClientError> {
        self.rpc.clone().reset_zero(self.device.clone()).await?;
        Ok(())
    }

    pub async fn zero(
        &self,
        translation: proto::Vector3,
        target: proto::Vector2,
        tracking_event: Option<common::events::TrackingEvent>,
    ) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .zero(proto::ZeroRequest {
                device: Some(self.device.clone()),
                translation: Some(translation),
                target: Some(target),
                tracking_event: tracking_event.map(Into::into),
            })
            .await?;
        Ok(())
    }

    pub async fn save_zero(&self) -> Result<(), ClientError> {
        self.rpc.clone().save_zero(self.device.clone()).await?;
        Ok(())
    }

    pub async fn clear_zero(&self) -> Result<(), ClientError> {
        self.rpc.clone().clear_zero(self.device.clone()).await?;
        Ok(())
    }

    pub async fn reset_shot_delay(&self) -> Result<u16, ClientError> {
        let reply = self
            .rpc
            .clone()
            .reset_shot_delay(self.device.clone())
            .await?
            .into_inner();
        Ok(u16::try_from(reply.delay_ms)
            .map_err(|_conversion_error| invalid_reply("shot delay reply exceeds u16"))?)
    }

    pub async fn get_shot_delay(&self) -> Result<u16, ClientError> {
        let reply = self
            .rpc
            .clone()
            .get_shot_delay(self.device.clone())
            .await?
            .into_inner();
        Ok(u16::try_from(reply.delay_ms)
            .map_err(|_conversion_error| invalid_reply("shot delay reply exceeds u16"))?)
    }

    pub async fn set_shot_delay(&self, delay_ms: u16) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .set_shot_delay(proto::SetShotDelayRequest {
                device: Some(self.device.clone()),
                delay_ms: delay_ms.into(),
            })
            .await?;
        Ok(())
    }

    pub async fn subscribe_shot_delay(
        &self,
    ) -> Result<impl futures::Stream<Item = Result<u16, RpcError>> + use<>, ClientError> {
        let stream = self
            .rpc
            .clone()
            .subscribe_shot_delay(proto::SubscribeShotDelayRequest {
                device: Some(self.device.clone()),
            })
            .await?
            .into_inner();
        Ok(stream.map(|item| {
            item.map_err(RpcError::from).and_then(|reply| {
                u16::try_from(reply.delay_ms)
                    .map_err(|_conversion_error| invalid_reply("shot delay reply exceeds u16"))
            })
        }))
    }

    pub async fn save_shot_delay(&self) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .save_shot_delay(self.device.clone())
            .await?;
        Ok(())
    }

    pub async fn set_device_name(&self, name: String) -> Result<(), ClientError> {
        self.rpc
            .clone()
            .set_device_name(proto::SetDeviceNameRequest {
                device: Some(self.device.clone()),
                name,
            })
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn malformed_events_are_skipped_without_losing_subsequent_events_or_rpc_errors() {
        let valid = proto::Event {
            event_oneof: Some(proto::event::EventOneof::Device(proto::DeviceEvent {
                device: Some(proto::Device {
                    uuid: vec![0; 6],
                    ..proto::Device::default()
                }),
                device_event_oneof: Some(proto::device_event::DeviceEventOneof::Impact(
                    proto::device_event::ImpactEvent { timestamp: 42 },
                )),
            })),
        };
        let mut events = event_stream(futures::stream::iter([
            Ok(proto::Event::default()),
            Ok(valid),
            Err(tonic::Status::unavailable("peer disconnected")),
        ]));
        let event = events
            .next()
            .await
            .expect("impact event")
            .expect("valid event");
        assert!(matches!(
            event,
            common::events::Event::DeviceEvent(common::events::DeviceEvent(
                _,
                common::events::DeviceEventKind::ImpactEvent(common::events::ImpactEvent {
                    timestamp: 42
                })
            ))
        ));
        let status = events
            .next()
            .await
            .expect("RPC error")
            .expect_err("error must not be skipped")
            .into_status();
        assert_eq!(status.code(), tonic::Code::Unavailable);
        assert!(events.next().await.is_none());
    }
}
