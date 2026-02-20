use futures::StreamExt;
use interprocess::local_socket::{
    tokio::prelude::LocalSocketStream, traits::tokio::Stream, GenericFilePath, GenericNamespaced,
    NameType as _, ToFsName, ToNsName,
};
use odyssey_hub_common as common;
use odyssey_hub_server_interface::{service_client::ServiceClient, DeviceListRequest};
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

    pub async fn get_device_list(&mut self) -> anyhow::Result<Vec<common::device::Device>> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(DeviceListRequest {});
            // for whatever insane reason get_device_list requires mutable service_client
            let response = service_client.get_device_list(request).await.unwrap();
            Ok(response
                .into_inner()
                .device_list
                .into_iter()
                .map(|d| d.into())
                .collect::<Vec<common::device::Device>>())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn bring_to_front(&mut self) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_server_interface::EmptyRequest {});
            service_client.bring_to_front(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn subscribe_device_list(
        &mut self,
    ) -> anyhow::Result<
        impl futures::Stream<Item = Result<Vec<common::device::Device>, tonic::Status>>,
    > {
        if let Some(service_client) = &mut self.service_client {
            let request =
                tonic::Request::new(odyssey_hub_server_interface::SubscribeDeviceListRequest {});
            let stream = service_client
                .subscribe_device_list(request)
                .await?
                .into_inner();

            Ok(stream.map(|item| {
                item.map(|reply| {
                    reply
                        .device_list
                        .into_iter()
                        .map(Into::into)
                        .collect::<Vec<common::device::Device>>()
                })
            }))
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn get_accessory_map(&mut self) -> anyhow::Result<common::accessory::AccessoryMap> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_server_interface::AccessoryMapRequest {});
            let response = service_client.get_accessory_map(request).await?;
            Ok(response.into_inner().into())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn subscribe_accessory_map(
        &mut self,
    ) -> anyhow::Result<
        impl futures::Stream<Item = Result<common::accessory::AccessoryMap, tonic::Status>>,
    > {
        if let Some(service_client) = &mut self.service_client {
            let request =
                tonic::Request::new(odyssey_hub_server_interface::SubscribeAccessoryMapRequest {});
            let stream = service_client
                .subscribe_accessory_map(request)
                .await?
                .into_inner();

            Ok(stream.map(|item| item.map(Into::into)))
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn subscribe_events(
        &mut self,
    ) -> anyhow::Result<impl futures::Stream<Item = Result<common::events::Event, tonic::Status>>>
    {
        if let Some(service_client) = &mut self.service_client {
            let request =
                tonic::Request::new(odyssey_hub_server_interface::SubscribeEventsRequest {});
            let stream = service_client.subscribe_events(request).await?.into_inner();

            Ok(stream.map(|item| item.map(Into::into)))
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn write_vendor(
        &mut self,
        device: common::device::Device,
        tag: u8,
        data: Vec<u8>,
    ) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_server_interface::WriteVendorRequest {
                device: Some(device.into()),
                tag: tag.into(),
                data,
            });
            service_client.write_vendor(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn read_register(
        &mut self,
        device: common::device::Device,
        port: u8,
        bank: u8,
        address: u8,
    ) -> anyhow::Result<u8> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_server_interface::ReadRegisterRequest {
                device: Some(device.into()),
                port: port as u32,
                bank: bank as u32,
                address: address as u32,
            });
            let reply = service_client.read_register(request).await?.into_inner();
            Ok(reply.data as u8)
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn write_register(
        &mut self,
        device: common::device::Device,
        port: u8,
        bank: u8,
        address: u8,
        data: u8,
    ) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_server_interface::WriteRegisterRequest {
                device: Some(device.into()),
                port: port as u32,
                bank: bank as u32,
                address: address as u32,
                data: data as u32,
            });
            service_client.write_register(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn read_config(
        &mut self,
        device: common::device::Device,
        kind: u8,
    ) -> anyhow::Result<u8> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_server_interface::ReadConfigRequest {
                device: Some(device.into()),
                kind: kind as u32,
            });
            let reply = service_client.read_config(request).await?.into_inner();
            Ok(reply.value as u8)
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn write_config(
        &mut self,
        device: common::device::Device,
        kind: u8,
        value: u8,
    ) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_server_interface::WriteConfigRequest {
                device: Some(device.into()),
                kind: kind as u32,
                value: value as u32,
            });
            service_client.write_config(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn start_pairing(
        &mut self,
        device: common::device::Device,
        timeout_ms: u32,
    ) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_server_interface::StartPairingRequest {
                device: Some(device.into()),
                timeout_ms,
            });
            service_client.start_pairing(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn cancel_pairing(&mut self, device: common::device::Device) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_server_interface::CancelPairingRequest {
                device: Some(device.into()),
            });
            service_client.cancel_pairing(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn clear_bond(&mut self, device: common::device::Device) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_server_interface::ClearBondRequest {
                device: Some(device.into()),
            });
            service_client.clear_bond(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn flash_settings(&mut self, device: common::device::Device) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_server_interface::FlashSettingsRequest {
                device: Some(device.into()),
            });
            service_client.flash_settings(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn reset_zero(&mut self, device: common::device::Device) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(device.into());
            service_client.reset_zero(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn zero(
        &mut self,
        device: common::device::Device,
        translation: odyssey_hub_server_interface::Vector3,
        target: odyssey_hub_server_interface::Vector2,
        tracking_event: Option<common::events::TrackingEvent>,
    ) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let tracking_event_pb = tracking_event.map(|te| {
                odyssey_hub_server_interface::device_event::TrackingEvent {
                    timestamp: te.timestamp,
                    aimpoint: Some(odyssey_hub_server_interface::Vector2::from(te.aimpoint)),
                    pose: Some(odyssey_hub_server_interface::Pose {
                        rotation: Some(odyssey_hub_server_interface::Matrix3x3::from(
                            te.pose.rotation,
                        )),
                        translation: Some(odyssey_hub_server_interface::Matrix3x1::from(
                            te.pose.translation,
                        )),
                    }),
                    distance: te.distance,
                    screen_id: te.screen_id,
                }
            });
            let request = tonic::Request::new(odyssey_hub_server_interface::ZeroRequest {
                device: Some(device.into()),
                translation: Some(translation.into()),
                target: Some(target.into()),
                tracking_event: tracking_event_pb,
            });
            service_client.zero(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn save_zero(&mut self, device: common::device::Device) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(device.into());
            service_client.save_zero(request).await?.into_inner();
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn clear_zero(&mut self, device: common::device::Device) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(device.into());
            service_client.clear_zero(request).await?.into_inner();
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn reset_shot_delay(
        &mut self,
        device: common::device::Device,
    ) -> anyhow::Result<u16> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(device.into());
            Ok(service_client
                .reset_shot_delay(request)
                .await?
                .into_inner()
                .delay_ms as u16)
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn get_shot_delay(&mut self, device: common::device::Device) -> anyhow::Result<u16> {
        if let Some(service_client) = &mut self.service_client {
            Ok(service_client
                .get_shot_delay(tonic::Request::new(device.into()))
                .await?
                .into_inner()
                .delay_ms as u16)
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn set_shot_delay(
        &mut self,
        device: common::device::Device,
        delay_ms: u16,
    ) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(odyssey_hub_server_interface::SetShotDelayRequest {
                device: Some(device.into()),
                delay_ms: delay_ms.into(),
            });
            service_client.set_shot_delay(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn subscribe_shot_delay(
        &mut self,
        device: common::device::Device,
    ) -> anyhow::Result<impl futures::Stream<Item = Result<u16, tonic::Status>>> {
        if let Some(service_client) = &mut self.service_client {
            let request =
                tonic::Request::new(odyssey_hub_server_interface::SubscribeShotDelayRequest {
                    device: Some(device.into()),
                });
            let stream = service_client
                .subscribe_shot_delay(request)
                .await?
                .into_inner();
            Ok(stream.map(|item| item.map(|r| r.delay_ms as u16)))
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn save_shot_delay(&mut self, device: common::device::Device) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            service_client
                .save_shot_delay(tonic::Request::new(device.into()))
                .await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn get_screen_info_by_id(&mut self, id: u8) -> anyhow::Result<common::ScreenInfo> {
        if let Some(service_client) = &mut self.service_client {
            let request =
                tonic::Request::new(odyssey_hub_server_interface::ScreenInfoByIdRequest {
                    id: id.into(),
                });
            Ok(service_client
                .get_screen_info_by_id(request)
                .await?
                .into_inner()
                .into())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn update_accessory_info_map(
        &mut self,
        map: common::accessory::AccessoryInfoMap,
    ) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request = tonic::Request::new(map.into());
            service_client.update_accessory_info_map(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    // -------------------- Dongle methods --------------------

    pub async fn subscribe_dongle_list(
        &mut self,
    ) -> anyhow::Result<impl futures::Stream<Item = Result<Vec<DongleInfo>, tonic::Status>>> {
        if let Some(service_client) = &mut self.service_client {
            let request =
                tonic::Request::new(odyssey_hub_server_interface::SubscribeDongleListRequest {});
            let stream = service_client
                .subscribe_dongle_list(request)
                .await?
                .into_inner();

            Ok(stream.map(|item| {
                item.map(|reply| {
                    reply
                        .dongle_list
                        .into_iter()
                        .map(|d| DongleInfo {
                            id: d.id,
                            protocol_version: d
                                .protocol_version
                                .map(|v| [v.major as u16, v.minor as u16, v.patch as u16])
                                .unwrap_or([0, 0, 0]),
                            control_protocol_version: d
                                .control_protocol_version
                                .map(|v| [v.major as u16, v.minor as u16, v.patch as u16])
                                .unwrap_or([0, 0, 0]),
                            firmware_version: d
                                .firmware_version
                                .map(|v| [v.major as u16, v.minor as u16, v.patch as u16])
                                .unwrap_or([0, 0, 0]),
                            connected_devices: d
                                .connected_devices
                                .iter()
                                .filter_map(|bytes| {
                                    if bytes.len() == 6 {
                                        let mut addr = [0u8; 6];
                                        addr.copy_from_slice(bytes);
                                        Some(addr)
                                    } else {
                                        None
                                    }
                                })
                                .collect(),
                            bonded_devices: d
                                .bonded_devices
                                .iter()
                                .filter_map(|bytes| {
                                    if bytes.len() == 6 {
                                        let mut addr = [0u8; 6];
                                        addr.copy_from_slice(bytes);
                                        Some(addr)
                                    } else {
                                        None
                                    }
                                })
                                .collect(),
                        })
                        .collect()
                })
            }))
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn start_dongle_pairing(
        &mut self,
        dongle_id: String,
        timeout_ms: u32,
    ) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request =
                tonic::Request::new(odyssey_hub_server_interface::StartDonglePairingRequest {
                    dongle_id,
                    timeout_ms,
                });
            service_client.start_dongle_pairing(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn cancel_dongle_pairing(&mut self, dongle_id: String) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request =
                tonic::Request::new(odyssey_hub_server_interface::CancelDonglePairingRequest {
                    dongle_id,
                });
            service_client.cancel_dongle_pairing(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }

    pub async fn clear_dongle_bonds(&mut self, dongle_id: String) -> anyhow::Result<()> {
        if let Some(service_client) = &mut self.service_client {
            let request =
                tonic::Request::new(odyssey_hub_server_interface::ClearDongleBondsRequest {
                    dongle_id,
                });
            service_client.clear_dongle_bonds(request).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("No service client"))
        }
    }
}

/// Dongle metadata returned by the dongle list subscription.
#[derive(Clone, Debug, PartialEq)]
pub struct DongleInfo {
    pub id: String,
    pub protocol_version: [u16; 3],         // mux endpoint protocol
    pub control_protocol_version: [u16; 3], // control endpoint protocol
    pub firmware_version: [u16; 3],
    pub connected_devices: Vec<[u8; 6]>,
    pub bonded_devices: Vec<[u8; 6]>,
}
