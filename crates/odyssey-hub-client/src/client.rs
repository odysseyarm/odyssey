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

fn devices_from_reply(reply: proto::DeviceListReply) -> Vec<common::device::Device> {
    reply.device_list.into_iter().map(Into::into).collect()
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
        Ok(devices_from_reply(reply))
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

        Ok(stream.map(|item| item.map_err(RpcError::from).map(devices_from_reply)))
    }

    pub async fn get_accessory_map(
        &mut self,
    ) -> Result<common::accessory::AccessoryMap, ClientError> {
        let reply = self
            .connected()?
            .get_accessory_map(proto::AccessoryMapRequest {})
            .await?
            .into_inner();
        Ok(reply.into())
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

        Ok(stream.map(|item| item.map_err(RpcError::from).map(Into::into)))
    }

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

        // An event that fails to convert is skipped rather than ending the
        // subscription.
        Ok(Box::pin(stream.filter_map(|item| async move {
            match item {
                Ok(event) => common::events::Event::try_from(event).ok().map(Ok),
                Err(status) => Some(Err(RpcError::from(status))),
            }
        })))
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
        Ok(reply.data as u8)
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
        Ok(reply.value as u8)
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
        Ok(reply.delay_ms as u16)
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
        Ok(reply.delay_ms as u16)
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
            item.map_err(RpcError::from)
                .map(|reply| reply.delay_ms as u16)
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
        Ok(reply.into())
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
            item.map_err(RpcError::from).map(|reply| {
                reply
                    .dongle_list
                    .into_iter()
                    .map(DongleInfo::from)
                    .collect()
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
