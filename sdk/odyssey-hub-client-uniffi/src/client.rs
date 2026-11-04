use std::sync::Arc;

use futures::StreamExt;
use odyssey_hub_client::client::RpcError;
use tokio::sync::{Mutex, RwLock};

use crate::ffi_common::*;

/// A gRPC failure with the original typed status retained as its error source.
#[derive(uniffi::Object, Debug, thiserror::Error)]
#[error("{}", .source.message())]
pub struct RpcFailure {
    #[source]
    source: tonic::Status,
}

#[uniffi::export]
impl RpcFailure {
    pub fn message(&self) -> String {
        self.source.message().to_string()
    }

    pub fn code(&self) -> u32 {
        self.source.code() as u32
    }
}

/// A local socket or transport failure, retaining the typed native error.
#[derive(uniffi::Object, Debug, thiserror::Error)]
#[error("{source}")]
pub struct ConnectionFailure {
    #[source]
    source: odyssey_hub_client::client::ClientError,
}

#[uniffi::export]
impl ConnectionFailure {
    pub fn message(&self) -> String {
        self.to_string()
    }
}

#[derive(uniffi::Enum, Debug, thiserror::Error)]
pub enum ClientError {
    #[error("Not connected to the server")]
    NotConnected,
    #[error("Stream ended unexpectedly")]
    StreamEnd,
    #[error("{source}")]
    RpcFailure {
        #[source]
        source: Arc<RpcFailure>,
    },
    #[error("{source}")]
    ConnectionFailure {
        #[source]
        source: Arc<ConnectionFailure>,
    },
    #[error("{message}")]
    InvalidArgument { message: String },
}

impl From<odyssey_hub_client::client::ClientError> for ClientError {
    fn from(error: odyssey_hub_client::client::ClientError) -> Self {
        match error {
            odyssey_hub_client::client::ClientError::Rpc(error) => error.into(),
            source => Self::ConnectionFailure {
                source: Arc::new(ConnectionFailure { source }),
            },
        }
    }
}

impl From<RpcError> for ClientError {
    fn from(error: RpcError) -> Self {
        Self::RpcFailure {
            source: Arc::new(RpcFailure {
                source: error.into_status(),
            }),
        }
    }
}

#[derive(uniffi::Object)]
pub struct Client {
    inner: Arc<RwLock<Option<odyssey_hub_client::client::Client>>>,
}

impl Client {
    async fn client(&self) -> Result<odyssey_hub_client::client::Client, ClientError> {
        self.inner
            .read()
            .await
            .clone()
            .ok_or(ClientError::NotConnected)
    }

    // Device validation precedes the connection check, matching the argument-first order of the RPC path.
    async fn device(
        &self,
        device: Device,
    ) -> Result<odyssey_hub_client::client::DeviceClient, ClientError> {
        let device = device.try_into()?;
        Ok(self.client().await?.device(device))
    }
}

#[derive(uniffi::Object)]
pub struct EventStream {
    pub inner: Arc<
        Mutex<
            futures::stream::BoxStream<
                'static,
                Result<odyssey_hub_common::events::Event, RpcError>,
            >,
        >,
    >,
}

#[derive(uniffi::Object)]
pub struct DeviceListStream {
    pub inner: Arc<
        Mutex<
            futures::stream::BoxStream<
                'static,
                Result<Vec<odyssey_hub_common::device::Device>, RpcError>,
            >,
        >,
    >,
}

#[derive(uniffi::Object)]
pub struct ShotDelayStream {
    pub inner: Arc<Mutex<futures::stream::BoxStream<'static, Result<u16, RpcError>>>>,
}

#[derive(uniffi::Object)]
pub struct AccessoryMapStream {
    pub inner: Arc<
        Mutex<
            futures::stream::BoxStream<
                'static,
                Result<odyssey_hub_common::accessory::AccessoryMap, RpcError>,
            >,
        >,
    >,
}

#[uniffi::export(async_runtime = "tokio")]
impl EventStream {
    #[uniffi::method]
    pub async fn next(&self) -> Result<Event, ClientError> {
        match self.inner.lock().await.next().await {
            Some(Ok(event)) => Ok(event.into()),
            Some(Err(error)) => Err(error.into()),
            None => Err(ClientError::StreamEnd),
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl DeviceListStream {
    #[uniffi::method]
    pub async fn next(&self) -> Result<Vec<Device>, ClientError> {
        match self.inner.lock().await.next().await {
            Some(msg) => match msg {
                Ok(device_list) => Ok(device_list
                    .into_iter()
                    .map(|d| odyssey_hub_common::device::Device::from(d).into())
                    .collect()),
                Err(error) => Err(error.into()),
            },
            _ => Err(ClientError::StreamEnd),
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl ShotDelayStream {
    #[uniffi::method]
    pub async fn next(&self) -> Result<u16, ClientError> {
        match self.inner.lock().await.next().await {
            Some(msg) => match msg {
                Ok(delay) => Ok(delay),
                Err(error) => Err(error.into()),
            },
            _ => Err(ClientError::StreamEnd),
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl AccessoryMapStream {
    #[uniffi::method]
    pub async fn next(&self) -> Result<Vec<crate::ffi_common::AccessoryMapEntry>, ClientError> {
        match self.inner.lock().await.next().await {
            Some(msg) => match msg {
                Ok(map) => {
                    let entries: Vec<crate::ffi_common::AccessoryMapEntry> = map
                        .into_iter()
                        .map(|(k6, (info, connected))| {
                            let mut buf = [0u8; 8];
                            buf[..6].copy_from_slice(&k6);
                            let key = u64::from_le_bytes(buf);
                            crate::ffi_common::AccessoryMapEntry {
                                key,
                                status: crate::ffi_common::AccessoryStatus {
                                    info: info.into(),
                                    connected,
                                },
                            }
                        })
                        .collect();
                    Ok(entries)
                }
                Err(error) => Err(error.into()),
            },
            _ => Err(ClientError::StreamEnd),
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Client {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
        }
    }

    #[uniffi::method]
    pub async fn connect(&self) -> Result<(), ClientError> {
        let client = odyssey_hub_client::client::Client::connect().await?;
        *self.inner.write().await = Some(client);
        Ok(())
    }

    #[uniffi::method]
    pub async fn get_device_list(&self) -> Result<Vec<Device>, ClientError> {
        self.client()
            .await?
            .get_device_list()
            .await
            .map_err(ClientError::from)
            .map(|dl| dl.into_iter().map(|d| d.into()).collect())
    }

    #[uniffi::method]
    pub async fn subscribe_device_list(&self) -> Result<DeviceListStream, ClientError> {
        let client = self.client().await?;

        match client.subscribe_device_list().await {
            Ok(stream) => Ok(DeviceListStream {
                inner: Arc::new(Mutex::new(Box::pin(stream))),
            }),
            Err(error) => Err(error.into()),
        }
    }

    #[uniffi::method]
    pub async fn get_accessory_map(
        &self,
    ) -> Result<Vec<crate::ffi_common::AccessoryMapEntry>, ClientError> {
        self.client()
            .await?
            .get_accessory_map()
            .await
            .map(|map| {
                map.into_iter()
                    .map(|(k6, (info, connected))| {
                        let mut buf = [0u8; 8];
                        buf[..6].copy_from_slice(&k6);
                        let key = u64::from_le_bytes(buf);
                        crate::ffi_common::AccessoryMapEntry {
                            key,
                            status: crate::ffi_common::AccessoryStatus {
                                info: info.into(),
                                connected,
                            },
                        }
                    })
                    .collect()
            })
            .map_err(ClientError::from)
    }

    #[uniffi::method]
    pub async fn subscribe_accessory_map(&self) -> Result<AccessoryMapStream, ClientError> {
        let client = self.client().await?;

        match client.subscribe_accessory_map().await {
            Ok(stream) => Ok(AccessoryMapStream {
                inner: Arc::new(Mutex::new(Box::pin(stream))),
            }),
            Err(error) => Err(error.into()),
        }
    }

    #[uniffi::method]
    pub async fn subscribe_events(&self) -> Result<EventStream, ClientError> {
        let client = self.client().await?;

        match client.subscribe_events().await {
            Ok(stream) => Ok(EventStream {
                inner: Arc::new(Mutex::new(Box::pin(stream))),
            }),
            Err(error) => Err(error.into()),
        }
    }

    #[uniffi::method]
    pub async fn subscribe_shot_delay(
        &self,
        device: Device,
    ) -> Result<ShotDelayStream, ClientError> {
        match self.device(device).await?.subscribe_shot_delay().await {
            Ok(stream) => Ok(ShotDelayStream {
                inner: Arc::new(Mutex::new(Box::pin(stream))),
            }),
            Err(error) => Err(error.into()),
        }
    }

    #[uniffi::method]
    pub async fn write_vendor(
        &self,
        device: Device,
        tag: u8,
        data: Vec<u8>,
    ) -> Result<(), ClientError> {
        self.device(device)
            .await?
            .write_vendor(tag, data)
            .await
            .map_err(ClientError::from)
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn reset_zero(&self, device: Device) -> Result<(), ClientError> {
        self.device(device)
            .await?
            .reset_zero()
            .await
            .map_err(ClientError::from)
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn zero(
        &self,
        device: Device,
        translation: crate::funny::Vector3f32,
        target: crate::funny::Vector2f32,
    ) -> Result<(), ClientError> {
        let translation = odyssey_hub_server_interface::Vector3 {
            x: translation.x,
            y: translation.y,
            z: translation.z,
        };
        let target = odyssey_hub_server_interface::Vector2 {
            x: target.x,
            y: target.y,
        };

        self.device(device)
            .await?
            .zero(translation, target, None)
            .await
            .map_err(ClientError::from)
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn reset_shot_delay(&self, device: Device) -> Result<u16, ClientError> {
        self.device(device)
            .await?
            .reset_shot_delay()
            .await
            .map_err(ClientError::from)
    }

    #[uniffi::method]
    pub async fn get_shot_delay(&self, device: Device) -> Result<u16, ClientError> {
        self.device(device)
            .await?
            .get_shot_delay()
            .await
            .map_err(ClientError::from)
    }

    #[uniffi::method]
    pub async fn set_shot_delay(&self, device: Device, delay_ms: u16) -> Result<(), ClientError> {
        self.device(device)
            .await?
            .set_shot_delay(delay_ms)
            .await
            .map_err(ClientError::from)
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn save_shot_delay(&self, device: Device) -> Result<(), ClientError> {
        self.device(device)
            .await?
            .save_shot_delay()
            .await
            .map_err(ClientError::from)
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn get_screen_info_by_id(&self, screen_id: u8) -> Result<ScreenInfo, ClientError> {
        self.client()
            .await?
            .get_screen_info_by_id(screen_id)
            .await
            .map_err(ClientError::from)
            .map(|info| info.into())
    }

    #[uniffi::method]
    pub async fn read_register(
        &self,
        device: Device,
        port: u8,
        bank: u8,
        address: u8,
    ) -> Result<u8, ClientError> {
        self.device(device)
            .await?
            .read_register(port, bank, address)
            .await
            .map_err(ClientError::from)
    }

    #[uniffi::method]
    pub async fn write_register(
        &self,
        device: Device,
        port: u8,
        bank: u8,
        address: u8,
        data: u8,
    ) -> Result<(), ClientError> {
        self.device(device)
            .await?
            .write_register(port, bank, address, data)
            .await
            .map_err(ClientError::from)
    }

    #[uniffi::method]
    pub async fn flash_settings(&self, device: Device) -> Result<(), ClientError> {
        self.device(device)
            .await?
            .flash_settings()
            .await
            .map_err(ClientError::from)
    }

    #[uniffi::method]
    pub async fn update_accessory_info_map(
        &self,
        entries: Vec<crate::ffi_common::AccessoryInfoMapEntry>,
    ) -> Result<(), ClientError> {
        let map: odyssey_hub_common::accessory::AccessoryInfoMap = entries
            .into_iter()
            .map(|entry| {
                if entry.key >> 48 != 0 {
                    return Err(ClientError::InvalidArgument {
                        message: "accessory key must fit in 48 bits".to_string(),
                    });
                }
                let bytes = entry.key.to_le_bytes();
                let key = [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]];
                Ok((key, entry.info.try_into()?))
            })
            .collect::<Result<_, ClientError>>()?;

        self.client()
            .await?
            .update_accessory_info_map(map)
            .await
            .map_err(ClientError::from)
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_errors_preserve_status_and_differ_from_eof() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let stream = EventStream {
            inner: Arc::new(Mutex::new(Box::pin(futures::stream::iter([Err(
                RpcError::from(tonic::Status::permission_denied("device is locked")),
            )])))),
        };

        runtime.block_on(async {
            match stream.next().await {
                Err(ClientError::RpcFailure { source }) => {
                    assert_eq!(source.code(), tonic::Code::PermissionDenied as u32);
                    assert_eq!(source.message(), "device is locked");
                    let status = std::error::Error::source(source.as_ref())
                        .expect("typed status source")
                        .downcast_ref::<tonic::Status>()
                        .expect("original gRPC status");
                    assert_eq!(status.code(), tonic::Code::PermissionDenied);
                }
                _ => panic!("expected an RPC failure, not EOF or NotConnected"),
            }
            assert!(matches!(stream.next().await, Err(ClientError::StreamEnd)));
        });
    }

    #[test]
    fn oversized_accessory_keys_are_rejected_before_rpc() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let result = runtime.block_on(Client::new().update_accessory_info_map(vec![
            AccessoryInfoMapEntry {
                key: 1 << 48,
                info: AccessoryInfo {
                    name: "accessory".to_string(),
                    ty: AccessoryType::DryFireMag,
                    assignment: None,
                },
            },
        ]));
        assert!(matches!(result, Err(ClientError::InvalidArgument { .. })));
    }

    #[test]
    fn unconnected_client_reports_not_connected() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let client = Client::new();
        assert!(matches!(
            runtime.block_on(client.get_device_list()),
            Err(ClientError::NotConnected)
        ));
        assert!(matches!(
            runtime.block_on(client.subscribe_events()),
            Err(ClientError::NotConnected)
        ));
    }
}
