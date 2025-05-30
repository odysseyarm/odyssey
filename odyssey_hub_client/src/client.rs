use interprocess::local_socket::{
    tokio::prelude::LocalSocketStream, traits::tokio::Stream, NameTypeSupport, ToFsName, ToNsName,
};
use odyssey_hub_service_interface::{service_client::ServiceClient, DeviceListRequest};
use tokio_util::sync::CancellationToken;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;

#[derive(Clone, Default)]
pub struct Client {
    pub end_token: CancellationToken,
    pub service_client: Option<ServiceClient<tonic::transport::Channel>>,
}

impl Client {
    pub async fn connect(&mut self) -> anyhow::Result<()> {
        let name = {
            use NameTypeSupport as Nts;
            match NameTypeSupport::query() {
                Nts::OnlyFs => "/tmp/odyhub.sock".to_fs_name().unwrap(),
                Nts::OnlyNs | Nts::Both => "@odyhub.sock".to_ns_name().unwrap(),
            }
        };

        // Await this here since we can't do a whole lot without a connection.
        // URI is ignored
        let channel = Endpoint::try_from("http://[::]:50051")?
            .connect_with_connector(service_fn(move |_: Uri| {
                let name = name.clone();
                async move {
                    let r = LocalSocketStream::connect(name).await?;
                    let r = odyssey_hub_service::service::LocalSocketStream::new(r);
                    std::io::Result::Ok(r)
                }
            }))
            .await?;

        self.service_client = Some(ServiceClient::new(channel));

        Ok(())
    }

    pub async fn get_device_list(
        &mut self,
    ) -> anyhow::Result<Vec<odyssey_hub_common::device::Device>> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(DeviceListRequest {});
            // for whatever insane reason get_device_list requires mutable service_client
            let response = service_client.get_device_list(request).await.unwrap();
            Ok(response
                .into_inner()
                .device_list
                .into_iter()
                .map(|d| d.into())
                .collect::<Vec<odyssey_hub_common::device::Device>>())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn poll(
        &mut self,
    ) -> anyhow::Result<tonic::Streaming<odyssey_hub_service_interface::PollReply>> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_service_interface::PollRequest {});
            Ok(service_client.poll(request).await?.into_inner())
        } else {
            Err(anyhow::anyhow!("No service client")).into()
        }
    }

    pub async fn write_vendor(
        &mut self,
        device: odyssey_hub_common::device::Device,
        tag: u8,
        data: Vec<u8>,
    ) -> anyhow::Result<odyssey_hub_service_interface::WriteVendorReply> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_service_interface::WriteVendorRequest {
                device: Some(device.into()),
                tag: tag.into(),
                data,
            });
            Ok(service_client.write_vendor(request).await?.into_inner())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn reset_zero(
        &mut self,
        device: odyssey_hub_common::device::Device,
    ) -> anyhow::Result<odyssey_hub_service_interface::ResetZeroReply> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(device.into());
            Ok(service_client.reset_zero(request).await?.into_inner())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn zero(
        &mut self,
        device: odyssey_hub_common::device::Device,
        translation: odyssey_hub_service_interface::Vector3,
        target: odyssey_hub_service_interface::Vector2,
    ) -> anyhow::Result<odyssey_hub_service_interface::ZeroReply> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_service_interface::ZeroRequest {
                device: Some(device.into()),
                translation: Some(translation.into()),
                target: Some(target.into()),
            });
            Ok(service_client.zero(request).await?.into_inner())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn clear_zero(
        &mut self,
        device: odyssey_hub_common::device::Device,
    ) -> anyhow::Result<odyssey_hub_service_interface::ClearZeroReply> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(device.into());
            Ok(service_client.clear_zero(request).await?.into_inner())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn get_screen_info_by_id(
        &mut self,
        id: u8,
    ) -> anyhow::Result<odyssey_hub_common::ScreenInfo> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_service_interface::ScreenInfoByIdRequest {
                id: id.into(),
            });
            Ok(service_client.get_screen_info_by_id(request).await?.into_inner().into())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }
}
