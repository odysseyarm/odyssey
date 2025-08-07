use std::sync::Arc;

use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use ats_cv::ScreenCalibration;
use odyssey_hub_common::{config, AccessoryInfoMap};
use odyssey_hub_server_interface::{
    service_server::Service, AccessoryMapRequest, DeviceListReply, DeviceListRequest, EmptyReply, GetShotDelayReply, ResetShotDelayReply, SetShotDelayRequest, SubscribeAccessoryMapRequest, SubscribeEventsRequest, Vector2
};
use tokio::{
    sync::{broadcast, mpsc},
};
use tokio_stream::wrappers::ReceiverStream;

use crate::device_tasks;

#[derive(Debug)]
pub struct Server {
    pub device_list: Arc<
        parking_lot::Mutex<
            Vec<(
                odyssey_hub_common::device::Device,
                ats_usb::device::UsbDevice,
                tokio::sync::mpsc::Sender<device_tasks::DeviceTaskMessage>,
            )>,
        >,
    >,
    pub event_sender: broadcast::Sender<odyssey_hub_common::events::Event>,
    pub screen_calibrations: Arc<
        ArcSwap<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
    pub device_offsets:
        Arc<tokio::sync::Mutex<std::collections::HashMap<u64, nalgebra::Isometry3<f32>>>>,
    pub device_shot_delays: std::sync::Mutex<std::collections::HashMap<u64, u16>>,
    pub accessory_map: Arc<std::sync::Mutex<odyssey_hub_common::AccessoryMap>>,
    pub accessory_map_sender: broadcast::Sender<odyssey_hub_common::AccessoryMap>,
    pub accessory_info_sender: tokio::sync::watch::Sender<AccessoryInfoMap>,
}

#[tonic::async_trait]
impl Service for Server {
    type SubscribeAccessoryMapStream = ReceiverStream<Result<odyssey_hub_server_interface::AccessoryMapReply, tonic::Status>>;
    type SubscribeEventsStream = ReceiverStream<Result<odyssey_hub_server_interface::Event, tonic::Status>>;

    async fn get_device_list(
        &self,
        _: tonic::Request<DeviceListRequest>,
    ) -> Result<tonic::Response<DeviceListReply>, tonic::Status> {
        let device_list = self
            .device_list
            .lock()
            .iter()
            .map(|d| d.0.clone().into())
            .collect();

        let reply = DeviceListReply { device_list };

        Ok(tonic::Response::new(reply))
    }

    async fn get_accessory_map(
        &self,
        _: tonic::Request<AccessoryMapRequest>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::AccessoryMapReply>, tonic::Status> {
        let accessory_map = self.accessory_map.lock().unwrap().clone().into();
        Ok(tonic::Response::new(accessory_map))
    }
    
    async fn subscribe_accessory_map(
        &self,
        _: tonic::Request<SubscribeAccessoryMapRequest>,
    ) -> Result<tonic::Response<Self::SubscribeAccessoryMapStream>, tonic::Status> {
        let (tx, rx) = mpsc::channel(12);
        let mut accessory_map_channel = self.accessory_map_sender.subscribe();
        tokio::spawn(async move {
            loop {
                let accessory_map = match accessory_map_channel.recv().await {
                    Ok(v) => v,
                    Err(broadcast::error::RecvError::Lagged(dropped)) => {
                        tracing::warn!("Dropping {dropped} accessory map notifications because the client is too slow");
                        accessory_map_channel = accessory_map_channel.resubscribe();
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let send_result = tx
                    .send(Ok(accessory_map.into()))
                    .await;
                if send_result.is_err() {
                    break;
                }
            }
        });
        Ok(tonic::Response::new(ReceiverStream::new(rx)))
    }

    async fn subscribe_events(
        &self,
        _: tonic::Request<SubscribeEventsRequest>,
    ) -> tonic::Result<tonic::Response<Self::SubscribeEventsStream>, tonic::Status> {
        let (tx, rx) = mpsc::channel(12);
        let mut event_channel = self.event_sender.subscribe();
        tokio::spawn(async move {
            loop {
                let event = match event_channel.recv().await {
                    Ok(v) => v,
                    Err(broadcast::error::RecvError::Lagged(dropped)) => {
                        tracing::warn!("Dropping {dropped} events because the client is too slow");
                        event_channel = event_channel.resubscribe();
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let send_result = tx
                    .send(Ok(event.into()))
                    .await;
                if send_result.is_err() {
                    break;
                }
            }
        });
        Ok(tonic::Response::new(ReceiverStream::new(rx)))
    }

    async fn write_vendor(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::WriteVendorRequest>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::EmptyReply>, tonic::Status> {
        let odyssey_hub_server_interface::WriteVendorRequest { device, tag, data } =
            request.into_inner();

        let device = device.unwrap().into();

        let d: ats_usb::device::UsbDevice;

        if let Some(_device) = self.device_list.lock().iter().find(|d| d.0 == device) {
            d = _device.1.clone();
        } else {
            return Err(tonic::Status::not_found("Device not found in device list"));
        }

        match d.write_vendor(tag as u8, data.as_slice()).await {
            Ok(_) => {}
            Err(e) => {
                return Err(tonic::Status::aborted(format!(
                    "Failed to write vendor: {}",
                    e
                )));
            }
        }

        Ok(tonic::Response::new(
            odyssey_hub_server_interface::EmptyReply {},
        ))
    }

    async fn reset_zero(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::Device>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::EmptyReply>, tonic::Status> {
        let device = request.into_inner().into();

        let s;

        if let Some(_device) = self.device_list.lock().iter().find(|d| d.0 == device) {
            s = _device.2.clone();
        } else {
            return Err(tonic::Status::not_found("Device not found in device list"));
        }

        match s.send(device_tasks::DeviceTaskMessage::ResetZero).await {
            Ok(_) => {}
            Err(e) => {
                return Err(tonic::Status::aborted(format!(
                    "Failed to send reset zero device task message: {}",
                    e
                )));
            }
        }

        Ok(tonic::Response::new(
            odyssey_hub_server_interface::EmptyReply {},
        ))
    }

    async fn zero(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::ZeroRequest>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::EmptyReply>, tonic::Status> {
        let odyssey_hub_server_interface::ZeroRequest {
            device,
            translation,
            target,
        } = request.into_inner();

        let device = device.unwrap().into();
        let translation = translation.unwrap();
        let translation = nalgebra::Translation3::new(translation.x, translation.y, translation.z);
        let target = target.unwrap();
        let target = nalgebra::Point2::new(target.x, target.y);

        let s;

        if let Some(_device) = self.device_list.lock().iter().find(|d| d.0 == device) {
            s = _device.2.clone();
        } else {
            return Err(tonic::Status::not_found("Device not found in device list"));
        }

        match s
            .send(device_tasks::DeviceTaskMessage::Zero(translation, target))
            .await
        {
            Ok(_) => {}
            Err(e) => {
                return Err(tonic::Status::aborted(format!(
                    "Failed to send zero device task message: {}",
                    e
                )));
            }
        }

        Ok(tonic::Response::new(
            odyssey_hub_server_interface::EmptyReply {},
        ))
    }

    async fn save_zero(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::Device>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::EmptyReply>, tonic::Status> {
        let device = request.into_inner().into();

        let s;

        if let Some(_device) = self.device_list.lock().iter().find(|d| d.0 == device) {
            s = _device.2.clone();
        } else {
            return Err(tonic::Status::not_found("Device not found in device list"));
        }

        match s.send(device_tasks::DeviceTaskMessage::SaveZero).await {
            Ok(_) => {}
            Err(e) => {
                return Err(tonic::Status::aborted(format!(
                    "Failed to send save zero device task message: {}",
                    e
                )));
            }
        }

        Ok(tonic::Response::new(
            odyssey_hub_server_interface::EmptyReply {},
        ))
    }

    async fn clear_zero(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::Device>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::EmptyReply>, tonic::Status> {
        let device = request.into_inner().into();

        let s;

        if let Some(_device) = self.device_list.lock().iter().find(|d| d.0 == device) {
            s = _device.2.clone();
        } else {
            return Err(tonic::Status::not_found("Device not found in device list"));
        }

        match s.send(device_tasks::DeviceTaskMessage::ClearZero).await {
            Ok(_) => {}
            Err(e) => {
                return Err(tonic::Status::aborted(format!(
                    "Failed to send clear zero device task message: {}",
                    e
                )));
            }
        }

        Ok(tonic::Response::new(
            odyssey_hub_server_interface::EmptyReply {},
        ))
    }

    async fn get_screen_info_by_id(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::ScreenInfoByIdRequest>,
    ) -> Result<tonic::Response<odyssey_hub_server_interface::ScreenInfoReply>, tonic::Status> {
        let odyssey_hub_server_interface::ScreenInfoByIdRequest { id } = request.into_inner();

        let screen_calibrations = self.screen_calibrations.load();

        let screen_calibration = screen_calibrations
            .iter()
            .find(|(i, _)| *i as u32 == id)
            .map(|(_, calibration)| calibration.clone());

        let reply = odyssey_hub_server_interface::ScreenInfoReply {
            id,
            bounds: {
                if let Some(screen_calibration) = screen_calibration {
                    let bounds = screen_calibration.bounds();
                    Some(odyssey_hub_server_interface::ScreenBounds {
                        tl: Some({
                            Vector2 {
                                x: bounds[0].x,
                                y: bounds[0].y,
                            }
                        }),
                        tr: Some({
                            Vector2 {
                                x: bounds[1].x,
                                y: bounds[1].y,
                            }
                        }),
                        bl: Some({
                            Vector2 {
                                x: bounds[2].x,
                                y: bounds[2].y,
                            }
                        }),
                        br: Some({
                            Vector2 {
                                x: bounds[3].x,
                                y: bounds[3].y,
                            }
                        }),
                    })
                } else {
                    None
                }
            },
        };

        Ok(tonic::Response::new(reply))
    }

    async fn reset_shot_delay(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::Device>,
    ) -> Result<tonic::Response<ResetShotDelayReply>, tonic::Status> {
        let device: odyssey_hub_common::device::Device = request.into_inner().into();

        let defaults = tokio::task::spawn_blocking(|| {
            odyssey_hub_common::config::device_shot_delays()
                .map_err(|e| anyhow::anyhow!("config error: {}", e))
        })
        .await
        .map_err(|e| tonic::Status::internal(format!("reload join error: {}", e)))?
        .map_err(|e| tonic::Status::internal(e.to_string()))?;

        let delay_ms = defaults.get(&device.uuid()).cloned().unwrap_or(0);
        self.device_shot_delays
            .lock()
            .unwrap()
            .insert(device.uuid(), delay_ms);

        if let Err(e) = self
            .event_sender
            .send(odyssey_hub_common::events::Event::DeviceEvent(
                odyssey_hub_common::events::DeviceEvent(
                    device.clone(),
                    odyssey_hub_common::events::DeviceEventKind::ShotDelayChangedEvent(delay_ms),
                ),
            ))
        {
            return Err(tonic::Status::internal(format!(
                "Failed to send event: {}",
                e
            )));
        }

        Ok(tonic::Response::new(ResetShotDelayReply {
            delay_ms: delay_ms as u32,
        }))
    }

    async fn get_shot_delay(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::Device>,
    ) -> Result<tonic::Response<GetShotDelayReply>, tonic::Status> {
        let device: odyssey_hub_common::device::Device = request.into_inner().into();

        let delay_ms = self
            .device_shot_delays
            .lock()
            .unwrap()
            .get(&device.uuid())
            .cloned()
            .unwrap_or(0)
            .into();

        Ok(tonic::Response::new(GetShotDelayReply { delay_ms }))
    }

    async fn set_shot_delay(
        &self,
        request: tonic::Request<SetShotDelayRequest>,
    ) -> Result<tonic::Response<EmptyReply>, tonic::Status> {
        let SetShotDelayRequest { device, delay_ms } = request.into_inner();
        let device: odyssey_hub_common::device::Device = device.unwrap().into();
        let delay_ms = delay_ms as u16;

        self.device_shot_delays
            .lock()
            .unwrap()
            .insert(device.uuid(), delay_ms);

        if let Err(e) = self
            .event_sender
            .send(odyssey_hub_common::events::Event::DeviceEvent(
                odyssey_hub_common::events::DeviceEvent(
                    device.clone(),
                    odyssey_hub_common::events::DeviceEventKind::ShotDelayChangedEvent(delay_ms),
                ),
            ))
        {
            return Err(tonic::Status::internal(format!(
                "Failed to send event: {}",
                e
            )));
        }

        Ok(tonic::Response::new(EmptyReply {}))
    }

    async fn save_shot_delay(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::Device>,
    ) -> Result<tonic::Response<EmptyReply>, tonic::Status> {
        let device: odyssey_hub_common::device::Device = request.into_inner().into();
        let uuid = device.uuid();

        let delay: u16 = {
            let guard = self.device_shot_delays.lock().unwrap();
            guard
                .get(&uuid)
                .cloned()
                .ok_or_else(|| tonic::Status::not_found("No shot-delay entry for this device"))?
        };

        tokio::task::spawn_blocking(move || {
            config::device_shot_delay_save(uuid, delay)
                .map_err(|e| anyhow::anyhow!("failed to save shot-delay: {}", e))
        })
        .await
        .map_err(|e| tonic::Status::internal(format!("blocking join error: {}", e)))?
        .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(EmptyReply {}))
    }
    
    async fn update_accessory_info_map(
        &self,
        request: tonic::Request<odyssey_hub_server_interface::AccessoryInfoMap>,
    ) -> Result<tonic::Response<EmptyReply>, tonic::Status> {
        let accessory_info_map = request.into_inner().into();

        self.accessory_info_sender.send_if_modified(|old_map| {
            if *old_map == accessory_info_map {
                false
            } else {
                *old_map = accessory_info_map.clone();
                true
            }
        });

        Ok(tonic::Response::new(EmptyReply {}))
    }
}
