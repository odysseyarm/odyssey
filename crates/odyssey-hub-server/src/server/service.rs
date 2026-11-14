use std::pin::Pin;
use std::task::{Context, Poll};

use odyssey_hub_common as common;
use odyssey_hub_server_interface::service_server::Service;
use odyssey_hub_server_interface::{
    AccessoryInfoMap, AccessoryMapReply, AccessoryMapRequest, CancelDonglePairingRequest,
    CancelPairingRequest, ClearBondRequest, ClearDongleBondsRequest, Device, DeviceListReply,
    DeviceListRequest, DongleListReply, EmptyReply, EmptyRequest, Event, FlashSettingsRequest,
    ExchangeVendorReply, ExchangeVendorRequest, GetShotDelayReply, ReadConfigReply, ReadConfigRequest,
    ReadRegisterReply, ReadRegisterRequest,
    MarkerOtaReply, MarkerOtaRequest, ResetShotDelayReply, ScreenBounds, ScreenInfoByIdRequest,
    ScreenInfoReply, SetDeviceNameRequest, SetShotDelayRequest, SetTransportModeRequest,
    StartDonglePairingRequest, StartPairingRequest, SubscribeAccessoryMapRequest,
    SubscribeDeviceListRequest, SubscribeDongleListRequest, SubscribeEventsRequest,
    SubscribeShotDelayRequest, TransportModeReply, Vector2, WriteConfigRequest,
    WriteRegisterRequest, WriteVendorRequest, ZeroRequest, device_event,
};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};
use tonic::{Response, Status};

use super::HubServer;
use super::device_uuid::{requested_uuid, uuid_from_bytes};
use super::hub_server::SUBSCRIPTION_BUFFER;
use super::rpc_error::RpcError;
use crate::accessories::ResetMarkerModeError;
use crate::device_tasks::{DeviceTaskMessage, DongleTaskMessage};

/// Capacity of the event stream. Tracking events arrive fast, so it is deeper
/// than the other subscriptions.
const EVENT_SUBSCRIPTION_BUFFER: usize = 128;

pub(crate) struct EventSubscription {
    events: ReceiverStream<Result<Event, Status>>,
    _demand: Option<crate::accessories::MarkerDemandGuard>,
}

impl futures::Stream for EventSubscription {
    type Item = Result<Event, Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        futures::Stream::poll_next(Pin::new(&mut self.get_mut().events), cx)
    }
}

/// Shot delay a device falls back to when reset.
const DEFAULT_SHOT_DELAY_MS: u16 = 0;

fn ok_empty() -> Response<EmptyReply> {
    Response::new(EmptyReply {})
}

/// Validate before narrowing. Truncation could enqueue a command for a different
/// sensor/register/config than the caller asked for.
fn request_byte(value: u32, maximum: u8, field: &'static str) -> Result<u8, RpcError> {
    u8::try_from(value)
        .ok()
        .filter(|value| *value <= maximum)
        .ok_or(RpcError::InvalidField(field))
}

fn tracking_event_from_proto(
    event: device_event::TrackingEvent,
) -> Result<common::events::TrackingEvent, RpcError> {
    let pose = event
        .pose
        .ok_or(RpcError::MissingField("tracking_event.pose"))?;
    Ok(common::events::TrackingEvent {
        timestamp: event.timestamp,
        aimpoint: event
            .aimpoint
            .ok_or(RpcError::MissingField("tracking_event.aimpoint"))?
            .into(),
        pose: common::events::Pose {
            rotation: pose
                .rotation
                .ok_or(RpcError::MissingField("tracking_event.pose.rotation"))?
                .into(),
            translation: pose
                .translation
                .ok_or(RpcError::MissingField("tracking_event.pose.translation"))?
                .into(),
        },
        distance: event.distance,
        screen_id: event.screen_id,
    })
}

#[tonic::async_trait]
impl Service for HubServer {
    // Concrete tokio_stream wrappers, so the crate needs no futures_core.
    type SubscribeDeviceListStream = ReceiverStream<Result<DeviceListReply, Status>>;
    type SubscribeAccessoryMapStream = ReceiverStream<Result<AccessoryMapReply, Status>>;
    type SubscribeAccessoryRadiosStream =
        ReceiverStream<Result<odyssey_hub_server_interface::AccessoryRadiosReply, Status>>;
    type SubscribeEventsStream = EventSubscription;
    type SubscribeShotDelayStream = ReceiverStream<Result<GetShotDelayReply, Status>>;
    type SubscribeDongleListStream = ReceiverStream<Result<DongleListReply, Status>>;
    type StreamImagesStream = crate::device_tasks::image_stream::ImageResponseStream;
    async fn get_device_list(
        &self,
        request: tonic::Request<DeviceListRequest>,
    ) -> Result<Response<DeviceListReply>, Status> {
        let _ = request;
        Ok(Response::new(self.device_list_reply()))
    }

    async fn subscribe_device_list(
        &self,
        request: tonic::Request<SubscribeDeviceListRequest>,
    ) -> Result<Response<Self::SubscribeDeviceListStream>, Status> {
        let _ = request;
        Ok(Response::new(self.device_list_updates()))
    }

    async fn get_accessory_map(
        &self,
        request: tonic::Request<AccessoryMapRequest>,
    ) -> Result<Response<AccessoryMapReply>, Status> {
        let _ = request;
        Ok(Response::new(AccessoryMapReply::from(
            self.accessory_map_snapshot(),
        )))
    }

    async fn subscribe_accessory_map(
        &self,
        request: tonic::Request<SubscribeAccessoryMapRequest>,
    ) -> Result<Response<Self::SubscribeAccessoryMapStream>, Status> {
        let _ = request;
        // The current map first, then every map the broadcast carries.
        let mut bcast = BroadcastStream::new(self.accessory_map_sender.subscribe());
        let init = self.accessory_map_snapshot();

        let (tx, rx) = mpsc::channel::<Result<AccessoryMapReply, Status>>(SUBSCRIPTION_BUFFER);
        let _ = tx.try_send(Ok(AccessoryMapReply::from(init)));

        // Detached on purpose: ends by itself when the client goes away.
        tokio::spawn(async move {
            loop {
                let item = tokio::select! {
                    () = tx.closed() => break,
                    item = bcast.next() => item,
                };
                let Some(item) = item else { break };
                match item {
                    Ok(map) => {
                        if tx.send(Ok(AccessoryMapReply::from(map))).await.is_err() {
                            break;
                        }
                    }
                    // The map is sent whole, so the next update supersedes anything
                    // missed. Dropping the subscription would freeze the accessory
                    // list for the rest of the session.
                    Err(BroadcastStreamRecvError::Lagged(n)) => {
                        tracing::debug!("accessory map subscriber lagged {n} updates");
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn get_accessory_radios(
        &self,
        request: tonic::Request<EmptyRequest>,
    ) -> Result<Response<odyssey_hub_server_interface::AccessoryRadiosReply>, Status> {
        let _ = request;
        Ok(Response::new(
            self.accessory_radios.snapshots.borrow().clone().into(),
        ))
    }

    async fn subscribe_accessory_radios(
        &self,
        request: tonic::Request<EmptyRequest>,
    ) -> Result<Response<Self::SubscribeAccessoryRadiosStream>, Status> {
        let _ = request;
        let mut snapshots = self.accessory_radios.snapshots.subscribe();
        let (tx, rx) = mpsc::channel(SUBSCRIPTION_BUFFER);
        let initial = snapshots.borrow_and_update().clone();
        let _ = tx.try_send(Ok(initial.into()));
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = tx.closed() => break,
                    changed = snapshots.changed() => if changed.is_err() { break; },
                }
                let snapshot = snapshots.borrow_and_update().clone();
                if tx.send(Ok(snapshot.into())).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn enroll_accessory(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::EnrollAccessoryRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let (address, mut info): ([u8; 6], common::accessory::AccessoryInfo) = request
            .into_inner()
            .try_into()
            .map_err(Status::invalid_argument)?;
        info.radio = self
            .accessory_radios
            .enroll(address, info.clone())
            .await
            .map_err(Status::failed_precondition)?;
        let _guard = self.accessory_config_lock.lock().await;
        let mut incoming = self.accessory_info_sender.borrow().clone();
        incoming.insert(address, info);
        common::config::accessory_map_save_async(&incoming)
            .await
            .map_err(|error| {
                Status::internal(format!("failed to save accessory info map: {error}"))
            })?;
        self.merge_accessory_info(incoming.clone());
        self.accessory_info_sender.send_replace(incoming);
        let _ = self
            .accessory_map_sender
            .send(self.accessory_map_snapshot());
        Ok(ok_empty())
    }

    async fn reset_marker_mode(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::ResetMarkerModeRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let address: [u8; 6] = request
            .into_inner()
            .try_into()
            .map_err(Status::invalid_argument)?;
        let configured = self.accessory_info_sender.borrow().clone();
        self.accessory_radios
            .reset_marker_mode(address, &configured)
            .await
            .map_err(|error| match error {
                ResetMarkerModeError::NotConfigured => Status::not_found(error.to_string()),
                ResetMarkerModeError::NotMarker | ResetMarkerModeError::Failed(_) => {
                    Status::failed_precondition(error.to_string())
                }
            })?;
        Ok(ok_empty())
    }
    async fn update_marker_ota(
        &self,
        request: tonic::Request<MarkerOtaRequest>,
    ) -> Result<Response<MarkerOtaReply>, Status> {
        let request = request.into_inner();
        let address_bytes = request.address.to_le_bytes();
        if address_bytes[6..] != [0; 2] {
            return Err(Status::invalid_argument("marker address exceeds 48 bits"));
        }
        let address = uuid_from_bytes(&address_bytes[..6])
            .map_err(|error| Status::invalid_argument(error.to_string()))?;
        let desktop_version = semver::Version::parse(&request.desktop_version)
            .map_err(|error| Status::invalid_argument(format!("invalid desktop version: {error}")))?;
        let configured = self.accessory_info_sender.borrow().clone();
        let version = self
            .accessory_radios
            .update_marker_ota(address, &configured, desktop_version)
            .await
            .map_err(Status::failed_precondition)?;
        Ok(Response::new(MarkerOtaReply { version }))
    }

    async fn update_accessory_info_map(
        &self,
        request: tonic::Request<AccessoryInfoMap>,
    ) -> Result<Response<EmptyReply>, Status> {
        let incoming: common::accessory::AccessoryInfoMap = request
            .into_inner()
            .try_into()
            .map_err(Status::invalid_argument)?;
        let _guard = self.accessory_config_lock.lock().await;

        common::config::accessory_map_save_async(&incoming)
            .await
            .map_err(|e| Status::internal(format!("failed to save accessory info map: {e}")))?;

        // Reloading gives the canonical form of what was saved.
        let fresh_result = common::config::accessory_map_async().await;
        let fresh = if fresh_result.is_corrupted() {
            tracing::warn!("Saved but file is corrupted on reload; using incoming");
            incoming
        } else {
            fresh_result.into_value()
        };

        // Publish canonical state before waking the accessory manager.
        self.merge_accessory_info(fresh.clone());
        self.accessory_info_sender.send_replace(fresh);
        // The whole status map goes out so the UI refreshes.
        let _ = self
            .accessory_map_sender
            .send(self.accessory_map_snapshot());

        Ok(ok_empty())
    }

    async fn get_shot_delay(
        &self,
        request: tonic::Request<Device>,
    ) -> Result<Response<GetShotDelayReply>, Status> {
        let uuid = uuid_from_bytes(&request.into_inner().uuid)?;
        let delay_ms = u32::from(self.shot_delay_or_default(uuid).await);
        Ok(Response::new(GetShotDelayReply { delay_ms }))
    }

    async fn subscribe_shot_delay(
        &self,
        request: tonic::Request<SubscribeShotDelayRequest>,
    ) -> Result<Response<Self::SubscribeShotDelayStream>, Status> {
        let uuid = requested_uuid(request.into_inner().device)?;

        let mut rx = self.subscribe_shot_delay_watch(uuid).await;
        let (tx, out) = mpsc::channel::<Result<GetShotDelayReply, Status>>(SUBSCRIPTION_BUFFER);

        let _ = tx.try_send(Ok(GetShotDelayReply {
            delay_ms: *rx.borrow_and_update(),
        }));
        // Detached on purpose: ends by itself when the client goes away.
        tokio::spawn(async move {
            loop {
                let changed = tokio::select! {
                    () = tx.closed() => break,
                    changed = rx.changed() => changed,
                };
                if changed.is_err() {
                    break;
                }
                let delay_ms = *rx.borrow_and_update();
                if tx.send(Ok(GetShotDelayReply { delay_ms })).await.is_err() {
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(out)))
    }

    async fn bring_to_front(
        &self,
        request: tonic::Request<EmptyRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let _ = request;
        if let Err(e) = self.app_message_sender.send(crate::Message::BringToFront) {
            tracing::error!("Failed to send BringToFront message: {}", e);
            return Err(Status::internal("Failed to bring window to front"));
        }
        Ok(ok_empty())
    }

    async fn set_shot_delay(
        &self,
        request: tonic::Request<SetShotDelayRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let SetShotDelayRequest { device, delay_ms } = request.into_inner();
        let uuid = requested_uuid(device)?;
        let ms = u16::try_from(delay_ms)
            .map_err(|_conversion_error| RpcError::InvalidField("delay_ms"))?;

        self.set_shot_delay_live(uuid, ms).await;
        // Also reaches a running device task, so the delay applies live.
        self.push_shot_delay_to_task(uuid, ms, "shot delay");

        Ok(ok_empty())
    }

    async fn reset_shot_delay(
        &self,
        request: tonic::Request<Device>,
    ) -> Result<Response<ResetShotDelayReply>, Status> {
        let uuid = uuid_from_bytes(&request.into_inner().uuid)?;

        self.set_shot_delay_live(uuid, DEFAULT_SHOT_DELAY_MS).await;
        self.push_shot_delay_to_task(uuid, DEFAULT_SHOT_DELAY_MS, "shot delay reset");

        Ok(Response::new(ResetShotDelayReply {
            delay_ms: u32::from(DEFAULT_SHOT_DELAY_MS),
        }))
    }

    async fn save_shot_delay(
        &self,
        request: tonic::Request<Device>,
    ) -> Result<Response<EmptyReply>, Status> {
        let uuid = uuid_from_bytes(&request.into_inner().uuid)?;
        #[cfg(feature = "sim")]
        if !crate::sim::persists(uuid) {
            return Ok(ok_empty());
        }
        let current = self.shot_delay_or_default(uuid).await;
        common::config::device_shot_delay_save_async(uuid, current)
            .await
            .map_err(|e| Status::internal(format!("failed to save delay: {e}")))?;
        Ok(ok_empty())
    }

    async fn get_screen_info_by_id(
        &self,
        request: tonic::Request<ScreenInfoByIdRequest>,
    ) -> Result<Response<ScreenInfoReply>, Status> {
        let id = request_byte(
            request.into_inner().id,
            odyssey_common::MAX_SCREEN_ID,
            "screen id",
        )?;
        let calibrations = self.screen_calibrations.load();
        let (_, calibration) = calibrations
            .iter()
            .find(|(screen_id, _)| *screen_id == id)
            .ok_or_else(|| Status::not_found("screen id"))?;
        let bounds = calibration
            .bounds()
            .ok_or_else(|| Status::failed_precondition("screen calibration has invalid bounds"))?;
        Ok(Response::new(ScreenInfoReply {
            id: u32::from(id),
            bounds: Some(ScreenBounds {
                tl: Some(Vector2::from(bounds[0].coords)),
                tr: Some(Vector2::from(bounds[1].coords)),
                bl: Some(Vector2::from(bounds[2].coords)),
                br: Some(Vector2::from(bounds[3].coords)),
            }),
        }))
    }

    async fn subscribe_events(
        &self,
        request: tonic::Request<SubscribeEventsRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        let req = request.into_inner();
        let screen_id = req
            .screen_id
            .map(|id| request_byte(id, odyssey_common::MAX_SCREEN_ID, "screen id"))
            .transpose()?;
        let demand = (!req.passive_tracking).then(|| self.marker_demand.acquire(screen_id));
        let mut rx = self.event_sender.subscribe();
        let (tx, out) = mpsc::channel::<Result<Event, Status>>(EVENT_SUBSCRIPTION_BUFFER);

        // Detached on purpose: ends by itself when the client goes away.
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    () = tx.closed() => break,
                    event = rx.recv() => event,
                };
                match event {
                    Ok(ev) => {
                        let encoded = Event::try_from(ev).map_err(|error| {
                            Status::internal(format!("failed to encode hub event: {error}"))
                        });
                        if tx.send(encoded).await.is_err() {
                            break;
                        }
                    }
                    // Tracking events can outrun a busy client. Skipping the dropped
                    // ones is correct. Tearing the subscription down would also lose
                    // every later impact and pairing result.
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!("event subscriber lagged {n} events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(Response::new(EventSubscription {
            events: ReceiverStream::new(out),
            _demand: demand,
        }))
    }

    async fn stream_images(
        &self,
        request: tonic::Request<
            tonic::Streaming<odyssey_hub_server_interface::StreamImagesRequest>,
        >,
    ) -> Result<Response<Self::StreamImagesStream>, Status> {
        let mut requests = request.into_inner();
        let request = requests
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("missing image stream request"))?;
        let camera = match request.camera {
            0 => odyssey_protocol::Port::Nf,
            1 => odyssey_protocol::Port::Wf,
            _ => return Err(Status::invalid_argument("invalid image camera")),
        };
        let fps = request_byte(request.fps, 15, "image FPS (expected 1..=15)")?;
        if fps == 0 {
            return Err(Status::invalid_argument(
                "image FPS must be between 1 and 15",
            ));
        }
        let task = self.device_task("stream_images", requested_uuid(request.device)?)?;
        let stream = task
            .request(|reply| DeviceTaskMessage::StartImages {
                config: odyssey_protocol::image::ImageStreamConfig { camera, fps },
                renewals: crate::device_tasks::ImageRenewals(Box::pin(
                    requests.map_while(|renewal| renewal.ok().map(drop)),
                )),
                reply,
            })
            .await??;
        Ok(Response::new(stream))
    }

    async fn reboot_device(
        &self,
        request: tonic::Request<Device>,
    ) -> Result<Response<EmptyReply>, Status> {
        let task =
            self.device_task("reboot_device", requested_uuid(Some(request.into_inner()))?)?;
        task.request(|reply| DeviceTaskMessage::Reboot { reply })
            .await?;
        Ok(ok_empty())
    }

    async fn write_vendor(
        &self,
        request: tonic::Request<WriteVendorRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let WriteVendorRequest { device, tag, data } = request.into_inner();
        let tag = request_byte(tag, 0xfe, "vendor tag (expected 0x81..=0xfe)")?;
        if tag < 0x81 {
            return Err(RpcError::InvalidField("vendor tag (expected 0x81..=0xfe)").into());
        }
        if data.len() > odyssey_protocol::VendorData::CAPACITY {
            return Err(Status::invalid_argument(format!(
                "vendor payload exceeds {} bytes",
                odyssey_protocol::VendorData::CAPACITY
            )));
        }
        let task = self.device_task("write_vendor", requested_uuid(device)?)?;
        task.send_now(DeviceTaskMessage::WriteVendor(tag, data))?;
        Ok(ok_empty())
    }

    async fn exchange_vendor(
        &self,
        request: tonic::Request<ExchangeVendorRequest>,
    ) -> Result<Response<ExchangeVendorReply>, Status> {
        let ExchangeVendorRequest { device, tag, data } = request.into_inner();
        if request_byte(tag, 0xfe, "vendor tag")? != odyssey_protocol::ota::VENDOR_TAG {
            return Err(Status::invalid_argument("unexpected vendor tag"));
        }
        odyssey_protocol::ota::Request::decode(&data)
            .map_err(|_| Status::invalid_argument("malformed OTA frame"))?;
        let task = self.device_task("exchange_vendor", requested_uuid(device)?)?;
        let status = task
            .request(|reply| DeviceTaskMessage::OtaExchange { frame: data, reply })
            .await?;
        Ok(Response::new(ExchangeVendorReply {
            data: status.to_vec(),
        }))
    }

    async fn read_register(
        &self,
        request: tonic::Request<ReadRegisterRequest>,
    ) -> Result<Response<ReadRegisterReply>, Status> {
        let inner = request.into_inner();
        let port = request_byte(inner.port, 1, "port")?;
        let bank = request_byte(inner.bank, u8::MAX, "bank")?;
        let address = request_byte(inner.address, u8::MAX, "address")?;
        let task = self.device_task("read_register", requested_uuid(inner.device)?)?;
        let data = task
            .request(|reply| DeviceTaskMessage::ReadRegister {
                port,
                bank,
                address,
                reply,
            })
            .await?;
        Ok(Response::new(ReadRegisterReply {
            data: u32::from(data),
        }))
    }

    async fn write_register(
        &self,
        request: tonic::Request<WriteRegisterRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let inner = request.into_inner();
        let port = request_byte(inner.port, 1, "port")?;
        let bank = request_byte(inner.bank, u8::MAX, "bank")?;
        let address = request_byte(inner.address, u8::MAX, "address")?;
        let data = request_byte(inner.data, u8::MAX, "data")?;
        let task = self.device_task("write_register", requested_uuid(inner.device)?)?;
        task.request(|reply| DeviceTaskMessage::WriteRegister {
            port,
            bank,
            address,
            data,
            reply,
        })
        .await?;
        Ok(ok_empty())
    }

    async fn read_config(
        &self,
        request: tonic::Request<ReadConfigRequest>,
    ) -> Result<Response<ReadConfigReply>, Status> {
        let inner = request.into_inner();
        let kind = request_byte(inner.kind, 1, "config kind")?;
        let task = self.device_task("read_config", requested_uuid(inner.device)?)?;
        let value = task
            .request(|reply| DeviceTaskMessage::ReadConfig { kind, reply })
            .await?;
        Ok(Response::new(ReadConfigReply {
            value: u32::from(value),
        }))
    }

    async fn write_config(
        &self,
        request: tonic::Request<WriteConfigRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let inner = request.into_inner();
        let kind = request_byte(inner.kind, 1, "config kind")?;
        let value = request_byte(inner.value, u8::MAX, "config value")?;
        let task = self.device_task("write_config", requested_uuid(inner.device)?)?;
        task.request(|reply| DeviceTaskMessage::WriteConfig { kind, value, reply })
            .await?;
        Ok(ok_empty())
    }

    async fn flash_settings(
        &self,
        request: tonic::Request<FlashSettingsRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let task = self.device_task(
            "flash_settings",
            requested_uuid(request.into_inner().device)?,
        )?;
        task.request(|reply| DeviceTaskMessage::FlashSettings { reply })
            .await?;
        Ok(ok_empty())
    }

    async fn start_pairing(
        &self,
        request: tonic::Request<StartPairingRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let inner = request.into_inner();
        let uuid = requested_uuid(inner.device)?;
        tracing::info!("start_pairing RPC for device {uuid:02x?}");
        let task = self.device_task("start_pairing", uuid)?;
        task.request(|reply| DeviceTaskMessage::StartPairing {
            timeout_ms: inner.timeout_ms,
            reply,
        })
        .await?;
        Ok(ok_empty())
    }

    async fn cancel_pairing(
        &self,
        request: tonic::Request<CancelPairingRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let uuid = requested_uuid(request.into_inner().device)?;
        tracing::info!("cancel_pairing RPC for device {uuid:02x?}");
        let task = self.device_task("cancel_pairing", uuid)?;
        task.request(|reply| DeviceTaskMessage::CancelPairing { reply })
            .await?;
        Ok(ok_empty())
    }

    async fn clear_bond(
        &self,
        request: tonic::Request<ClearBondRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let uuid = requested_uuid(request.into_inner().device)?;
        tracing::info!("clear_bond RPC for device {uuid:02x?}");
        let task = self.device_task("clear_bond", uuid)?;
        task.request(|reply| DeviceTaskMessage::ClearBond { reply })
            .await?;
        Ok(ok_empty())
    }

    async fn set_transport_mode(
        &self,
        request: tonic::Request<SetTransportModeRequest>,
    ) -> Result<Response<TransportModeReply>, Status> {
        let inner = request.into_inner();
        let uuid = requested_uuid(inner.device)?;
        tracing::info!(
            "set_transport_mode RPC for device {uuid:02x?}, usb_mode={}",
            inner.usb_mode
        );
        let task = self.device_task("set_transport_mode", uuid)?;
        let usb_mode = task
            .request(|reply| DeviceTaskMessage::SetTransportMode {
                usb_mode: inner.usb_mode,
                reply,
            })
            .await
            .inspect_err(|error| tracing::warn!("set_transport_mode: {error}"))?;
        tracing::info!("set_transport_mode: reply ok usb_mode={usb_mode}");
        Ok(Response::new(TransportModeReply { usb_mode }))
    }

    async fn subscribe_dongle_list(
        &self,
        request: tonic::Request<SubscribeDongleListRequest>,
    ) -> Result<Response<Self::SubscribeDongleListStream>, Status> {
        let _ = request;
        Ok(Response::new(self.dongle_list_updates()))
    }

    async fn start_dongle_pairing(
        &self,
        request: tonic::Request<StartDonglePairingRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let inner = request.into_inner();
        let task = self.dongle_task(&inner.dongle_id)?;
        task.request(|reply| DongleTaskMessage::StartPairing {
            timeout_ms: inner.timeout_ms,
            reply,
        })
        .await?;
        Ok(ok_empty())
    }

    async fn cancel_dongle_pairing(
        &self,
        request: tonic::Request<CancelDonglePairingRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let task = self.dongle_task(&request.into_inner().dongle_id)?;
        task.request(|reply| DongleTaskMessage::CancelPairing { reply })
            .await?;
        Ok(ok_empty())
    }

    async fn clear_dongle_bonds(
        &self,
        request: tonic::Request<ClearDongleBondsRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let task = self.dongle_task(&request.into_inner().dongle_id)?;
        task.request(|reply| DongleTaskMessage::ClearBonds { reply })
            .await?;
        Ok(ok_empty())
    }

    async fn set_device_name(
        &self,
        request: tonic::Request<SetDeviceNameRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let SetDeviceNameRequest { device, name } = request.into_inner();
        let uuid = requested_uuid(device)?;

        // The firmware stores the name in flash and updates its BLE advertising.
        let task = self.device_task("set_device_name", uuid)?;
        task.request(|reply| DeviceTaskMessage::SetDeviceName {
            name: name.clone(),
            reply,
        })
        .await
        .inspect_err(|error| tracing::warn!("set_device_name: {error}"))?;

        self.device_list.rename(uuid, &name);
        let _ = self.device_list_change_sender.send(());

        Ok(ok_empty())
    }

    async fn clear_zero(
        &self,
        request: tonic::Request<Device>,
    ) -> Result<Response<EmptyReply>, Status> {
        let task = self.device_task("clear_zero", uuid_from_bytes(&request.into_inner().uuid)?)?;
        task.send_now(DeviceTaskMessage::ClearZero)?;
        Ok(ok_empty())
    }

    async fn reset_zero(
        &self,
        request: tonic::Request<Device>,
    ) -> Result<Response<EmptyReply>, Status> {
        let task = self.device_task("reset_zero", uuid_from_bytes(&request.into_inner().uuid)?)?;
        task.send_now(DeviceTaskMessage::ResetZero)?;
        Ok(ok_empty())
    }

    async fn save_zero(
        &self,
        request: tonic::Request<Device>,
    ) -> Result<Response<EmptyReply>, Status> {
        let task = self.device_task("save_zero", uuid_from_bytes(&request.into_inner().uuid)?)?;
        task.send_now(DeviceTaskMessage::SaveZero)?;
        Ok(ok_empty())
    }

    async fn zero(
        &self,
        request: tonic::Request<ZeroRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let ZeroRequest {
            device,
            translation,
            target,
            tracking_event,
        } = request.into_inner();

        let uuid = requested_uuid(device)?;

        let translation: nalgebra::Vector3<f32> = translation
            .ok_or(RpcError::MissingField("translation"))?
            .into();
        let target: nalgebra::Vector2<f32> = target.ok_or(RpcError::MissingField("target"))?.into();
        let tracking_event = tracking_event.map(tracking_event_from_proto).transpose()?;

        let task = self.device_task("zero", uuid)?;
        task.send_now(DeviceTaskMessage::Zero(
            nalgebra::Translation3::from(translation),
            nalgebra::Point2::from(target),
            tracking_event,
        ))?;
        Ok(ok_empty())
    }
}
