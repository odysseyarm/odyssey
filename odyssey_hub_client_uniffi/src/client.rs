use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::{Mutex, RwLock};

use crate::ffi_common::*;

#[derive(uniffi::Object, Debug, thiserror::Error)]
#[error("{e:?}")] // default message is from anyhow.
pub struct AnyhowError {
    e: uniffi::deps::anyhow::Error,
}

#[uniffi::export]
impl AnyhowError {
    fn anyhow_message(&self) -> String {
        self.to_string()
    }
}

impl From<uniffi::deps::anyhow::Error> for AnyhowError {
    fn from(e: uniffi::deps::anyhow::Error) -> Self {
        Self { e }
    }
}

#[derive(uniffi::Enum, Debug, thiserror::Error)]
pub enum ClientError {
    #[error("Not connected to the server")]
    NotConnected,
    #[error("Stream ended unexpectedly")]
    StreamEnd,
}

#[derive(uniffi::Object)]
pub struct Client {
    inner: Arc<RwLock<odyssey_hub_client::client::Client>>,
}

#[derive(uniffi::Object)]
pub struct EventStream {
    pub inner: Arc<
        Mutex<
            futures::stream::BoxStream<
                'static,
                Result<odyssey_hub_common::events::Event, tonic::Status>,
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
                Result<Vec<odyssey_hub_common::device::Device>, tonic::Status>,
            >,
        >,
    >,
}

#[derive(uniffi::Object)]
pub struct ShotDelayStream {
    pub inner: Arc<Mutex<futures::stream::BoxStream<'static, Result<u16, tonic::Status>>>>,
}

#[derive(uniffi::Object)]
pub struct AccessoryMapStream {
    pub inner: Arc<
        Mutex<
            futures::stream::BoxStream<
                'static,
                Result<odyssey_hub_common::accessory::AccessoryMap, tonic::Status>,
            >,
        >,
    >,
}

#[uniffi::export(async_runtime = "tokio")]
impl EventStream {
    #[uniffi::method]
    pub async fn next(&self) -> Result<Event, ClientError> {
        if let Some(msg) = self.inner.lock().await.next().await {
            match msg {
                Ok(event) => Ok(odyssey_hub_common::events::Event::from(event).into()),
                Err(_) => Err(ClientError::StreamEnd),
            }
        } else {
            Err(ClientError::StreamEnd)
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl DeviceListStream {
    #[uniffi::method]
    pub async fn next(&self) -> Result<Vec<Device>, ClientError> {
        if let Some(msg) = self.inner.lock().await.next().await {
            match msg {
                Ok(device_list) => Ok(device_list
                    .into_iter()
                    .map(|d| odyssey_hub_common::device::Device::from(d).into())
                    .collect()),
                Err(_) => Err(ClientError::StreamEnd),
            }
        } else {
            Err(ClientError::StreamEnd)
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl ShotDelayStream {
    #[uniffi::method]
    pub async fn next(&self) -> Result<u16, ClientError> {
        if let Some(msg) = self.inner.lock().await.next().await {
            match msg {
                Ok(delay) => Ok(delay),
                Err(_) => Err(ClientError::StreamEnd),
            }
        } else {
            Err(ClientError::StreamEnd)
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl AccessoryMapStream {
    #[uniffi::method]
    pub async fn next(&self) -> Result<Vec<crate::ffi_common::AccessoryMapEntry>, ClientError> {
        if let Some(msg) = self.inner.lock().await.next().await {
            match msg {
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
                Err(_) => Err(ClientError::StreamEnd),
            }
        } else {
            Err(ClientError::StreamEnd)
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Client {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(odyssey_hub_client::client::Client::default())),
        }
    }

    #[uniffi::method]
    pub async fn connect(&self) -> Result<(), Arc<AnyhowError>> {
        self.inner
            .write()
            .await
            .connect()
            .await
            .map_err(|e| Arc::new(e.into()))
    }

    #[uniffi::method]
    pub async fn get_device_list(&self) -> Result<Vec<Device>, Arc<AnyhowError>> {
        self.inner
            .write()
            .await
            .get_device_list()
            .await
            .map_err(|e| Arc::new(e.into()))
            .map(|dl| dl.into_iter().map(|d| d.into()).collect())
    }

    #[uniffi::method]
    pub async fn subscribe_device_list(&self) -> Result<DeviceListStream, ClientError> {
        let mut client = self.inner.read().await.clone();

        match client.subscribe_device_list().await {
            Ok(stream) => Ok(DeviceListStream {
                inner: Arc::new(Mutex::new(Box::pin(stream))),
            }),
            Err(_) => Err(ClientError::NotConnected),
        }
    }

    #[uniffi::method]
    pub async fn get_accessory_map(
        &self,
    ) -> Result<Vec<crate::ffi_common::AccessoryMapEntry>, Arc<AnyhowError>> {
        self.inner
            .write()
            .await
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
            .map_err(|e| Arc::new(e.into()))
    }

    #[uniffi::method]
    pub async fn subscribe_accessory_map(&self) -> Result<AccessoryMapStream, ClientError> {
        let mut client = self.inner.read().await.clone();

        match client.subscribe_accessory_map().await {
            Ok(stream) => Ok(AccessoryMapStream {
                inner: Arc::new(Mutex::new(Box::pin(stream))),
            }),
            Err(_) => Err(ClientError::NotConnected),
        }
    }

    #[uniffi::method]
    pub async fn subscribe_events(&self) -> Result<EventStream, ClientError> {
        let mut client = self.inner.read().await.clone();

        match client.subscribe_events().await {
            Ok(stream) => Ok(EventStream {
                inner: Arc::new(Mutex::new(Box::pin(stream))),
            }),
            Err(_) => Err(ClientError::NotConnected),
        }
    }

    #[uniffi::method]
    pub async fn subscribe_shot_delay(
        &self,
        device: Device,
    ) -> Result<ShotDelayStream, ClientError> {
        let mut client = self.inner.read().await.clone();

        match client.subscribe_shot_delay(device.into()).await {
            Ok(stream) => Ok(ShotDelayStream {
                inner: Arc::new(Mutex::new(Box::pin(stream))),
            }),
            Err(_) => Err(ClientError::NotConnected),
        }
    }

    #[uniffi::method]
    pub async fn write_vendor(
        &self,
        device: Device,
        tag: u8,
        data: Vec<u8>,
    ) -> Result<(), ClientError> {
        self.inner
            .write()
            .await
            .write_vendor(device.into(), tag, data)
            .await
            .map_err(|_| ClientError::NotConnected)
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn reset_zero(&self, device: Device) -> Result<(), ClientError> {
        self.inner
            .write()
            .await
            .reset_zero(device.into())
            .await
            .map_err(|_| ClientError::NotConnected)
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

        self.inner
            .write()
            .await
            .zero(device.into(), translation, target)
            .await
            .map_err(|_| ClientError::NotConnected)
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn reset_shot_delay(&self, device: Device) -> Result<u16, ClientError> {
        self.inner
            .write()
            .await
            .reset_shot_delay(device.into())
            .await
            .map_err(|_| ClientError::NotConnected)
    }

    // TODO ClientError does not encompass all possible errors
    #[uniffi::method]
    pub async fn get_shot_delay(&self, device: Device) -> Result<u16, ClientError> {
        self.inner
            .write()
            .await
            .get_shot_delay(device.into())
            .await
            .map_err(|_| ClientError::NotConnected)
    }

    #[uniffi::method]
    pub async fn set_shot_delay(&self, device: Device, delay_ms: u16) -> Result<(), ClientError> {
        self.inner
            .write()
            .await
            .set_shot_delay(device.into(), delay_ms)
            .await
            .map_err(|_| ClientError::NotConnected)
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn save_shot_delay(&self, device: Device) -> Result<(), ClientError> {
        self.inner
            .write()
            .await
            .save_shot_delay(device.into())
            .await
            .map_err(|_| ClientError::NotConnected)
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn get_screen_info_by_id(&self, screen_id: u8) -> Result<ScreenInfo, ClientError> {
        self.inner
            .write()
            .await
            .get_screen_info_by_id(screen_id)
            .await
            .map_err(|_| ClientError::NotConnected)
            .map(|info| info.into())
    }

    #[uniffi::method]
    pub async fn update_accessory_info_map(
        &self,
        entries: Vec<crate::ffi_common::AccessoryInfoMapEntry>,
    ) -> Result<(), ClientError> {
        let map: odyssey_hub_common::accessory::AccessoryInfoMap = entries
            .into_iter()
            .map(|e| {
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&e.key.to_le_bytes());
                let k6: [u8; 6] = buf[..6].try_into().unwrap();
                (k6, e.info.into())
            })
            .collect();

        self.inner
            .write()
            .await
            .update_accessory_info_map(map)
            .await
            .map_err(|_| ClientError::NotConnected)
            .map(|_| ())
    }
}
