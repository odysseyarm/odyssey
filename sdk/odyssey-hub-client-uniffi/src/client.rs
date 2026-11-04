use std::sync::Arc;

use futures::StreamExt;
use odyssey_hub_client::client::RpcError;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

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

#[derive(uniffi::Object, Default)]
pub struct Client {
    inner: Arc<RwLock<Option<odyssey_hub_client::client::Client>>>,
    closed: CancellationToken,
}

impl Client {
    async fn client(&self) -> Result<odyssey_hub_client::client::Client, ClientError> {
        if self.closed.is_cancelled() {
            return Err(ClientError::NotConnected);
        }
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

    async fn until_closed<T, E: Into<ClientError>>(
        &self,
        call: impl Future<Output = Result<T, E>>,
    ) -> Result<T, ClientError> {
        tokio::select! {
            biased;
            () = self.closed.cancelled() => Err(ClientError::NotConnected),
            result = call => result.map_err(Into::into),
        }
    }
}

async fn next_until_closed<T>(
    closed: &CancellationToken,
    inner: &Mutex<futures::stream::BoxStream<'static, T>>,
) -> Option<T> {
    tokio::select! {
        biased;
        () = closed.cancelled() => None,
        item = async { inner.lock().await.next().await } => item,
    }
}

/// A hub subscription shared between the foreign handle and its pending `next()` calls.
pub type SharedStream<T> = Arc<Mutex<futures::stream::BoxStream<'static, Result<T, RpcError>>>>;

#[derive(uniffi::Object)]
pub struct EventStream {
    pub inner: SharedStream<odyssey_hub_common::events::Event>,
    closed: CancellationToken,
}

#[derive(uniffi::Object)]
pub struct DeviceListStream {
    pub inner: SharedStream<Vec<odyssey_hub_common::device::Device>>,
    closed: CancellationToken,
}

#[derive(uniffi::Object)]
pub struct ShotDelayStream {
    pub inner: SharedStream<u16>,
    closed: CancellationToken,
}

#[derive(uniffi::Object)]
pub struct AccessoryMapStream {
    pub inner: SharedStream<odyssey_hub_common::accessory::AccessoryMap>,
    closed: CancellationToken,
}

#[uniffi::export(async_runtime = "tokio")]
impl EventStream {
    #[uniffi::method]
    pub async fn next(&self) -> Result<Event, ClientError> {
        match next_until_closed(&self.closed, &self.inner).await {
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
        match next_until_closed(&self.closed, &self.inner).await {
            Some(msg) => match msg {
                Ok(device_list) => Ok(device_list.into_iter().map(Into::into).collect()),
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
        match next_until_closed(&self.closed, &self.inner).await {
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
        match next_until_closed(&self.closed, &self.inner).await {
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
        Self::default()
    }

    /// Ends every stream created by this client with `StreamEnd` and makes every pending and
    /// later call fail with `NotConnected`. Terminal and idempotent.
    ///
    /// Call it before the host unloads the code that awaits these calls (e.g. Unity domain
    /// reload, interpreter shutdown): a call still pending afterwards calls back into unloaded code.
    #[uniffi::method]
    pub fn close(&self) {
        self.closed.cancel();
    }

    #[uniffi::method]
    pub async fn connect(&self) -> Result<(), ClientError> {
        self.until_closed(async {
            let client = odyssey_hub_client::client::Client::connect().await?;
            let mut inner = self.inner.write().await;
            if self.closed.is_cancelled() {
                return Err(ClientError::NotConnected);
            }
            *inner = Some(client);
            Ok(())
        })
        .await
    }

    #[uniffi::method]
    pub async fn get_device_list(&self) -> Result<Vec<Device>, ClientError> {
        self.until_closed(self.client().await?.get_device_list())
            .await
            .map(|dl| dl.into_iter().map(|d| d.into()).collect())
    }

    #[uniffi::method]
    pub async fn subscribe_device_list(&self) -> Result<DeviceListStream, ClientError> {
        let client = self.client().await?;

        match self.until_closed(client.subscribe_device_list()).await {
            Ok(stream) => Ok(DeviceListStream {
                inner: Arc::new(Mutex::new(Box::pin(stream))),
                closed: self.closed.clone(),
            }),
            Err(error) => Err(error),
        }
    }

    #[uniffi::method]
    pub async fn get_accessory_map(
        &self,
    ) -> Result<Vec<crate::ffi_common::AccessoryMapEntry>, ClientError> {
        self.until_closed(self.client().await?.get_accessory_map())
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
    }

    #[uniffi::method]
    pub async fn subscribe_accessory_map(&self) -> Result<AccessoryMapStream, ClientError> {
        let client = self.client().await?;

        match self.until_closed(client.subscribe_accessory_map()).await {
            Ok(stream) => Ok(AccessoryMapStream {
                inner: Arc::new(Mutex::new(Box::pin(stream))),
                closed: self.closed.clone(),
            }),
            Err(error) => Err(error),
        }
    }

    #[uniffi::method]
    pub async fn subscribe_events(&self) -> Result<EventStream, ClientError> {
        let client = self.client().await?;

        match self.until_closed(client.subscribe_events()).await {
            Ok(stream) => Ok(EventStream {
                inner: Arc::new(Mutex::new(Box::pin(stream))),
                closed: self.closed.clone(),
            }),
            Err(error) => Err(error),
        }
    }

    #[uniffi::method]
    pub async fn subscribe_shot_delay(
        &self,
        device: Device,
    ) -> Result<ShotDelayStream, ClientError> {
        let device = self.device(device).await?;
        match self.until_closed(device.subscribe_shot_delay()).await {
            Ok(stream) => Ok(ShotDelayStream {
                inner: Arc::new(Mutex::new(Box::pin(stream))),
                closed: self.closed.clone(),
            }),
            Err(error) => Err(error),
        }
    }

    #[uniffi::method]
    pub async fn write_vendor(
        &self,
        device: Device,
        tag: u8,
        data: Vec<u8>,
    ) -> Result<(), ClientError> {
        self.until_closed(self.device(device).await?.write_vendor(tag, data))
            .await
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn reset_zero(&self, device: Device) -> Result<(), ClientError> {
        self.until_closed(self.device(device).await?.reset_zero())
            .await
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

        self.until_closed(self.device(device).await?.zero(translation, target, None))
            .await
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn reset_shot_delay(&self, device: Device) -> Result<u16, ClientError> {
        self.until_closed(self.device(device).await?.reset_shot_delay())
            .await
    }

    #[uniffi::method]
    pub async fn get_shot_delay(&self, device: Device) -> Result<u16, ClientError> {
        self.until_closed(self.device(device).await?.get_shot_delay())
            .await
    }

    #[uniffi::method]
    pub async fn set_shot_delay(&self, device: Device, delay_ms: u16) -> Result<(), ClientError> {
        self.until_closed(self.device(device).await?.set_shot_delay(delay_ms))
            .await
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn save_shot_delay(&self, device: Device) -> Result<(), ClientError> {
        self.until_closed(self.device(device).await?.save_shot_delay())
            .await
            .map(|_| ())
    }

    #[uniffi::method]
    pub async fn get_screen_info_by_id(&self, screen_id: u8) -> Result<ScreenInfo, ClientError> {
        self.until_closed(self.client().await?.get_screen_info_by_id(screen_id))
            .await
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
        self.until_closed(
            self.device(device)
                .await?
                .read_register(port, bank, address),
        )
        .await
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
        self.until_closed(
            self.device(device)
                .await?
                .write_register(port, bank, address, data),
        )
        .await
    }

    #[uniffi::method]
    pub async fn flash_settings(&self, device: Device) -> Result<(), ClientError> {
        self.until_closed(self.device(device).await?.flash_settings())
            .await
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

        self.until_closed(self.client().await?.update_accessory_info_map(map))
            .await
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
            closed: CancellationToken::new(),
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

    #[test]
    fn close_ends_pending_streams_and_later_calls() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        let client = Client::new();
        let stream = Arc::new(EventStream {
            inner: Arc::new(Mutex::new(futures::stream::pending().boxed())),
            closed: client.closed.clone(),
        });

        runtime.block_on(async {
            let first = tokio::spawn({
                let stream = stream.clone();
                async move { stream.next().await }
            });
            let queued = tokio::spawn({
                let stream = stream.clone();
                async move { stream.next().await }
            });
            tokio::task::yield_now().await;
            client.close();
            client.close();
            for pending in [first, queued] {
                let result = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
                    .await
                    .expect("next() ends after close")
                    .expect("next() task");
                assert!(matches!(result, Err(ClientError::StreamEnd)));
            }
            let connect = tokio::time::timeout(std::time::Duration::from_secs(5), client.connect())
                .await
                .expect("connect() ends after close");
            assert!(matches!(connect, Err(ClientError::NotConnected)));
            assert!(client.inner.read().await.is_none());
        });
    }
}
