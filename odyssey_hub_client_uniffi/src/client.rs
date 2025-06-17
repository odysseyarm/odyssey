use std::sync::Arc;

use tokio::sync::RwLock;

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

#[derive(uniffi::Record)]
pub struct PollEventResult {
    pub event: Option<Event>,
    pub error: Option<ClientError>,
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
    pub async fn get_device_list(&self) -> Result<Vec<DeviceRecord>, Arc<AnyhowError>> {
        self.inner
            .write()
            .await
            .get_device_list()
            .await
            .map_err(|e| Arc::new(e.into()))
            .map(|dl| dl.into_iter().map(|d| d.into()).collect())
    }

    #[uniffi::method]
    pub async fn poll_event(&self) -> Result<PollEventResult, ClientError> {
        let mut client = self.inner.read().await.clone();

        match client.poll().await {
            Ok(mut stream) => {
                if let Ok(msg) = stream.message().await {
                    match msg {
                        Some(reply) => {
                            if let Some(event) = reply.event {
                                Ok(PollEventResult {
                                    event: Some(odyssey_hub_common::events::Event::from(event).into()),
                                    error: None,
                                })
                            } else {
                                Ok(PollEventResult {
                                    event: None,
                                    error: None,
                                })
                            }
                        }
                        None => {
                            Ok(PollEventResult {
                                event: None,
                                error: Some(ClientError::StreamEnd),
                            })
                        }
                    }
                } else {
                    Ok(PollEventResult {
                        event: None,
                        error: Some(ClientError::StreamEnd),
                    })
                }
            }
            Err(_) => {
                Err(ClientError::NotConnected)
            }
        }
    }

    #[uniffi::method]
    pub async fn stop_stream(&self) {
        self.inner.write().await.end_token.cancel();
    }

    #[uniffi::method]
    pub async fn write_vendor(
        &self,
        device: DeviceRecord,
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
    pub async fn reset_zero(&self, device: DeviceRecord) -> Result<(), ClientError> {
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
        device: DeviceRecord,
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
    pub async fn get_screen_info_by_id(&self, screen_id: u8) -> Result<ScreenInfo, ClientError> {
        self.inner
            .write()
            .await
            .get_screen_info_by_id(screen_id)
            .await
            .map_err(|_| ClientError::NotConnected)
            .map(|info| info.into())
    }
}
