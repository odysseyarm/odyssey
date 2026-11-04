use futures::StreamExt as _;
use interprocess::local_socket::tokio::prelude::LocalSocketStream;
use interprocess::local_socket::traits::tokio::Stream as _;
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, NameType as _, ToFsName as _, ToNsName as _,
};
use odyssey_hub_common as common;
use odyssey_hub_server_interface as proto;
use odyssey_hub_server_interface::service_client::ServiceClient;
use tokio_util::sync::CancellationToken;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

pub use crate::dongle::DongleInfo;
pub use crate::error::{ClientError, RpcError};

/// Connection to the hub's local gRPC socket.
///
/// Every RPC method fails with [`ClientError::NotConnected`] until
/// [`Client::connect`] has succeeded. The generated gRPC client needs `&mut`
/// for every call, hence `&mut self` throughout.
#[derive(Clone, Debug, Default)]
pub struct Client {
    pub end_token: CancellationToken,
    pub service_client: Option<ServiceClient<Channel>>,
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
    end_token: CancellationToken,
) -> impl futures::Stream<Item = Result<common::events::Event, RpcError>> + Unpin {
    // Malformed events do not terminate the subscription; cancellation does,
    // even if the server is idle and sends nothing further.
    Box::pin(
        stream
            .take_until(end_token.cancelled_owned())
            .filter_map(|item| async move {
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
            }),
    )
}

impl Client {
    pub async fn connect(&mut self) -> Result<(), ClientError> {
        let name = if GenericNamespaced::is_supported() {
            "@odyhub.sock".to_ns_name::<GenericNamespaced>()?
        } else {
            "/tmp/odyhub.sock".to_fs_name::<GenericFilePath>()?
        };

        // Await this here since we can't do a whole lot without a connection.
        // URI is ignored
        let channel = Endpoint::try_from("http://[::]:50051")?
            .connect_with_connector(service_fn(move |_: Uri| {
                let name = name.clone();
                async move {
                    let r = LocalSocketStream::connect(name).await?;
                    std::io::Result::Ok(r)
                }
            }))
            .await?;

        self.service_client = Some(ServiceClient::new(channel));

        Ok(())
    }

    fn connected(&mut self) -> Result<&mut ServiceClient<Channel>, ClientError> {
        self.service_client
            .as_mut()
            .ok_or(ClientError::NotConnected)
    }

    pub async fn get_device_list(&mut self) -> Result<Vec<common::device::Device>, ClientError> {
        let reply = self
            .connected()?
            .get_device_list(proto::DeviceListRequest {})
            .await?
            .into_inner();
        Ok(devices_from_reply(reply)?)
    }

    pub async fn bring_to_front(&mut self) -> Result<(), ClientError> {
        self.connected()?
            .bring_to_front(proto::EmptyRequest {})
            .await?;
        Ok(())
    }

    pub async fn subscribe_device_list(
        &mut self,
    ) -> Result<
        impl futures::Stream<Item = Result<Vec<common::device::Device>, RpcError>> + use<>,
        ClientError,
    > {
        let stream = self
            .connected()?
            .subscribe_device_list(proto::SubscribeDeviceListRequest {})
            .await?
            .into_inner();

        Ok(stream.map(|item| item.map_err(RpcError::from).and_then(devices_from_reply)))
    }

    pub async fn get_accessory_map(
        &mut self,
    ) -> Result<common::accessory::AccessoryMap, ClientError> {
        let reply = self
            .connected()?
            .get_accessory_map(proto::AccessoryMapRequest {})
            .await?
            .into_inner();
        Ok(reply.try_into().map_err(invalid_reply)?)
    }

    pub async fn subscribe_accessory_map(
        &mut self,
    ) -> Result<
        impl futures::Stream<Item = Result<common::accessory::AccessoryMap, RpcError>> + use<>,
        ClientError,
    > {
        let stream = self
            .connected()?
            .subscribe_accessory_map(proto::SubscribeAccessoryMapRequest {})
            .await?
            .into_inner();

        Ok(stream.map(|item| {
            item.map_err(RpcError::from)
                .and_then(|reply| reply.try_into().map_err(invalid_reply))
        }))
    }

    /// Streams hub events until the peer closes or [`Client::end_token`] is cancelled.
    pub async fn subscribe_events(
        &mut self,
    ) -> Result<
        impl futures::Stream<Item = Result<common::events::Event, RpcError>> + Unpin + use<>,
        ClientError,
    > {
        let stream = self
            .connected()?
            .subscribe_events(proto::SubscribeEventsRequest {})
            .await?
            .into_inner();

        Ok(event_stream(stream, self.end_token.clone()))
    }

    pub async fn write_vendor(
        &mut self,
        device: common::device::Device,
        tag: u8,
        data: Vec<u8>,
    ) -> Result<(), ClientError> {
        self.connected()?
            .write_vendor(proto::WriteVendorRequest {
                device: Some(device.into()),
                tag: tag.into(),
                data,
            })
            .await?;
        Ok(())
    }

    pub async fn read_register(
        &mut self,
        device: common::device::Device,
        port: u8,
        bank: u8,
        address: u8,
    ) -> Result<u8, ClientError> {
        let reply = self
            .connected()?
            .read_register(proto::ReadRegisterRequest {
                device: Some(device.into()),
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
        &mut self,
        device: common::device::Device,
        port: u8,
        bank: u8,
        address: u8,
        data: u8,
    ) -> Result<(), ClientError> {
        self.connected()?
            .write_register(proto::WriteRegisterRequest {
                device: Some(device.into()),
                port: u32::from(port),
                bank: u32::from(bank),
                address: u32::from(address),
                data: u32::from(data),
            })
            .await?;
        Ok(())
    }

    pub async fn read_config(
        &mut self,
        device: common::device::Device,
        kind: u8,
    ) -> Result<u8, ClientError> {
        let reply = self
            .connected()?
            .read_config(proto::ReadConfigRequest {
                device: Some(device.into()),
                kind: u32::from(kind),
            })
            .await?
            .into_inner();
        Ok(u8::try_from(reply.value)
            .map_err(|_conversion_error| invalid_reply("config reply exceeds u8"))?)
    }

    pub async fn write_config(
        &mut self,
        device: common::device::Device,
        kind: u8,
        value: u8,
    ) -> Result<(), ClientError> {
        self.connected()?
            .write_config(proto::WriteConfigRequest {
                device: Some(device.into()),
                kind: u32::from(kind),
                value: u32::from(value),
            })
            .await?;
        Ok(())
    }

    pub async fn start_pairing(
        &mut self,
        device: common::device::Device,
        timeout_ms: u32,
    ) -> Result<(), ClientError> {
        self.connected()?
            .start_pairing(proto::StartPairingRequest {
                device: Some(device.into()),
                timeout_ms,
            })
            .await?;
        Ok(())
    }

    pub async fn cancel_pairing(
        &mut self,
        device: common::device::Device,
    ) -> Result<(), ClientError> {
        self.connected()?
            .cancel_pairing(proto::CancelPairingRequest {
                device: Some(device.into()),
            })
            .await?;
        Ok(())
    }

    pub async fn clear_bond(&mut self, device: common::device::Device) -> Result<(), ClientError> {
        self.connected()?
            .clear_bond(proto::ClearBondRequest {
                device: Some(device.into()),
            })
            .await?;
        Ok(())
    }

    pub async fn set_transport_mode(
        &mut self,
        device: common::device::Device,
        usb_mode: bool,
    ) -> Result<bool, ClientError> {
        let reply = self
            .connected()?
            .set_transport_mode(proto::SetTransportModeRequest {
                device: Some(device.into()),
                usb_mode,
            })
            .await?
            .into_inner();
        Ok(reply.usb_mode)
    }

    pub async fn flash_settings(
        &mut self,
        device: common::device::Device,
    ) -> Result<(), ClientError> {
        self.connected()?
            .flash_settings(proto::FlashSettingsRequest {
                device: Some(device.into()),
            })
            .await?;
        Ok(())
    }

    pub async fn reset_zero(&mut self, device: common::device::Device) -> Result<(), ClientError> {
        self.connected()?
            .reset_zero(proto::Device::from(device))
            .await?;
        Ok(())
    }

    pub async fn zero(
        &mut self,
        device: common::device::Device,
        translation: proto::Vector3,
        target: proto::Vector2,
        tracking_event: Option<common::events::TrackingEvent>,
    ) -> Result<(), ClientError> {
        self.connected()?
            .zero(proto::ZeroRequest {
                device: Some(device.into()),
                translation: Some(translation),
                target: Some(target),
                tracking_event: tracking_event.map(Into::into),
            })
            .await?;
        Ok(())
    }

    pub async fn save_zero(&mut self, device: common::device::Device) -> Result<(), ClientError> {
        self.connected()?
            .save_zero(proto::Device::from(device))
            .await?;
        Ok(())
    }

    pub async fn clear_zero(&mut self, device: common::device::Device) -> Result<(), ClientError> {
        self.connected()?
            .clear_zero(proto::Device::from(device))
            .await?;
        Ok(())
    }

    pub async fn reset_shot_delay(
        &mut self,
        device: common::device::Device,
    ) -> Result<u16, ClientError> {
        let reply = self
            .connected()?
            .reset_shot_delay(proto::Device::from(device))
            .await?
            .into_inner();
        Ok(u16::try_from(reply.delay_ms)
            .map_err(|_conversion_error| invalid_reply("shot delay reply exceeds u16"))?)
    }

    pub async fn get_shot_delay(
        &mut self,
        device: common::device::Device,
    ) -> Result<u16, ClientError> {
        let reply = self
            .connected()?
            .get_shot_delay(proto::Device::from(device))
            .await?
            .into_inner();
        Ok(u16::try_from(reply.delay_ms)
            .map_err(|_conversion_error| invalid_reply("shot delay reply exceeds u16"))?)
    }

    pub async fn set_shot_delay(
        &mut self,
        device: common::device::Device,
        delay_ms: u16,
    ) -> Result<(), ClientError> {
        self.connected()?
            .set_shot_delay(proto::SetShotDelayRequest {
                device: Some(device.into()),
                delay_ms: delay_ms.into(),
            })
            .await?;
        Ok(())
    }

    pub async fn subscribe_shot_delay(
        &mut self,
        device: common::device::Device,
    ) -> Result<impl futures::Stream<Item = Result<u16, RpcError>> + use<>, ClientError> {
        let stream = self
            .connected()?
            .subscribe_shot_delay(proto::SubscribeShotDelayRequest {
                device: Some(device.into()),
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

    pub async fn save_shot_delay(
        &mut self,
        device: common::device::Device,
    ) -> Result<(), ClientError> {
        self.connected()?
            .save_shot_delay(proto::Device::from(device))
            .await?;
        Ok(())
    }

    pub async fn get_screen_info_by_id(
        &mut self,
        id: u8,
    ) -> Result<common::ScreenInfo, ClientError> {
        let reply = self
            .connected()?
            .get_screen_info_by_id(proto::ScreenInfoByIdRequest { id: id.into() })
            .await?
            .into_inner();
        Ok(reply.try_into().map_err(invalid_reply)?)
    }

    pub async fn update_accessory_info_map(
        &mut self,
        map: common::accessory::AccessoryInfoMap,
    ) -> Result<(), ClientError> {
        self.connected()?
            .update_accessory_info_map(proto::AccessoryInfoMap::from(map))
            .await?;
        Ok(())
    }

    // -------------------- Dongle methods --------------------

    pub async fn subscribe_dongle_list(
        &mut self,
    ) -> Result<impl futures::Stream<Item = Result<Vec<DongleInfo>, RpcError>> + use<>, ClientError>
    {
        let stream = self
            .connected()?
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
        &mut self,
        dongle_id: String,
        timeout_ms: u32,
    ) -> Result<(), ClientError> {
        self.connected()?
            .start_dongle_pairing(proto::StartDonglePairingRequest {
                dongle_id,
                timeout_ms,
            })
            .await?;
        Ok(())
    }

    pub async fn cancel_dongle_pairing(&mut self, dongle_id: String) -> Result<(), ClientError> {
        self.connected()?
            .cancel_dongle_pairing(proto::CancelDonglePairingRequest { dongle_id })
            .await?;
        Ok(())
    }

    pub async fn clear_dongle_bonds(&mut self, dongle_id: String) -> Result<(), ClientError> {
        self.connected()?
            .clear_dongle_bonds(proto::ClearDongleBondsRequest { dongle_id })
            .await?;
        Ok(())
    }

    pub async fn set_device_name(
        &mut self,
        device: common::device::Device,
        name: String,
    ) -> Result<(), ClientError> {
        self.connected()?
            .set_device_name(proto::SetDeviceNameRequest {
                device: Some(device.into()),
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
    async fn cancelling_an_idle_event_subscription_ends_it() {
        let end_token = CancellationToken::new();
        let mut events = event_stream(futures::stream::pending(), end_token.clone());
        end_token.cancel();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), events.next())
                .await
                .expect("cancelled idle subscription must end")
                .is_none()
        );
    }

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
        let mut events = event_stream(
            futures::stream::iter([
                Ok(proto::Event::default()),
                Ok(valid),
                Err(tonic::Status::unavailable("peer disconnected")),
            ]),
            CancellationToken::new(),
        );
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
