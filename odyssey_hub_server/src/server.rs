use std::{collections::HashMap, sync::Arc};

use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use ats_common::ScreenCalibration;
use nalgebra::Isometry3;
use odyssey_hub_common as common;
use parking_lot::Mutex as ParkingMutex;
use tokio::{
    sync::{broadcast, mpsc, watch, RwLock},
    task::JoinHandle,
};
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};
use tokio_stream::StreamExt;
use tonic::{Response, Status};

use crate::device_tasks::DeviceTaskMessage;

// -- Interface (tonic) --
use iface::service_server::Service;
use iface::Vector2;
use iface::*; // proto types re-exported from iface
use odyssey_hub_server_interface as iface;

/// gRPC service state
pub struct Server {
    /// Live device tuples pushed by lib.rs/device_tasks.
    /// NOTE: We also expose SubscribeDeviceList; we derive the snapshot from here.
    pub device_list:
        Arc<ParkingMutex<Vec<(common::device::Device, mpsc::Sender<DeviceTaskMessage>)>>>,

    /// Pub/Sub of low-level events used by existing pipeline.
    pub event_sender: broadcast::Sender<common::events::Event>,

    /// Notifies subscribers when device list changes (Connect/Disconnect)
    pub device_list_change_sender: broadcast::Sender<()>,

    /// Static data referenced by device_handlers (unchanged here, but kept to satisfy lib.rs construction)
    pub screen_calibrations: Arc<
        ArcSwap<
            ArrayVec<(u8, ScreenCalibration<f32>), { (ats_common::MAX_SCREEN_ID + 1) as usize }>,
        >,
    >,
    pub device_offsets: Arc<tokio::sync::Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,

    /// Shot delay store (uuid→delay), plus per-uuid watchers for SubscribeShotDelay.
    pub device_shot_delays: Arc<RwLock<HashMap<[u8; 6], u16>>>, // seeded by lib.rs at startup
    pub shot_delay_watch: Arc<RwLock<HashMap<[u8; 6], watch::Sender<u32>>>>,

    /// Accessory info/state
    pub accessory_map:
        Arc<std::sync::Mutex<HashMap<[u8; 6], (common::accessory::AccessoryInfo, bool)>>>,
    pub accessory_map_sender: broadcast::Sender<common::accessory::AccessoryMap>,

    /// Message sender to communicate with the desktop app
    pub app_message_sender: mpsc::UnboundedSender<crate::Message>,
    pub accessory_info_sender: watch::Sender<HashMap<[u8; 6], common::accessory::AccessoryInfo>>, // incoming updates
}

impl Server {
    // -------------------- Device list helpers --------------------
    fn device_snapshot(&self) -> Vec<common::device::Device> {
        self.device_list
            .lock()
            .iter()
            .map(|(d, _tx)| d.clone())
            .collect()
    }

    fn device_list_stream(
        &self,
    ) -> (
        mpsc::Receiver<Result<DeviceListReply, Status>>,
        JoinHandle<()>,
    ) {
        let mut change_rx = self.device_list_change_sender.subscribe();
        let (tx, rx) = mpsc::channel::<Result<DeviceListReply, Status>>(16);

        // Send initial snapshot
        let snapshot = self.device_snapshot();
        let _ = tx.try_send(Ok(DeviceListReply {
            device_list: snapshot.into_iter().map(|d| d.into()).collect(),
        }));

        // Clone only what the task needs so the future is 'static
        let device_list = self.device_list.clone();
        let handle = tokio::spawn(async move {
            loop {
                match change_rx.recv().await {
                    Ok(()) => {
                        // Build a fresh snapshot and push
                        let snap = {
                            let guard = device_list.lock();
                            guard.iter().map(|(d, _tx)| d.clone()).collect::<Vec<_>>()
                        };
                        if tx
                            .send(Ok(DeviceListReply {
                                device_list: snap.into_iter().map(|d| d.into()).collect(),
                            }))
                            .await
                            .is_err()
                        {
                            break; // client gone
                        }
                    }
                    Err(_) => break, // broadcast closed
                }
            }
        });
        (rx, handle)
    }

    // -------------------- Shot delay helpers --------------------
    async fn get_or_default_shot_delay(&self, uuid: [u8; 6]) -> u16 {
        if let Some(v) = self.device_shot_delays.read().await.get(&uuid).copied() {
            return v;
        }
        common::config::device_shot_delays_async()
            .await
            .into_value()
            .get(&uuid)
            .copied()
            .unwrap_or(0)
    }

    async fn set_shot_delay_live(&self, uuid: [u8; 6], ms: u16) {
        self.device_shot_delays.write().await.insert(uuid, ms);
        let mut g = self.shot_delay_watch.write().await;
        if let Some(tx) = g.get(&uuid) {
            let _ = tx.send(ms as u32);
        } else {
            let (tx, _rx) = watch::channel(ms as u32);
            let _ = tx.send(ms as u32);
            g.insert(uuid, tx);
        }
    }

    async fn subscribe_shot_delay_watch(&self, uuid: [u8; 6]) -> watch::Receiver<u32> {
        let cur = self.get_or_default_shot_delay(uuid).await as u32;
        let mut g = self.shot_delay_watch.write().await;
        if let Some(tx) = g.get(&uuid) {
            tx.subscribe()
        } else {
            let (tx, rx) = watch::channel(cur);
            g.insert(uuid, tx);
            rx
        }
    }

    async fn persist_shot_delay(&self, uuid: [u8; 6], ms: u16) -> Result<(), Status> {
        common::config::device_shot_delay_save_async(uuid, ms)
            .await
            .map_err(|e| Status::internal(format!("failed to save delay: {e}")))
    }

    // -------------------- Accessory map helpers --------------------
    fn accessory_map_snapshot(&self) -> common::accessory::AccessoryMap {
        // Convert HashMap<[u8;6], (info, connected)> to AccessoryMap type alias
        self.accessory_map
            .lock()
            .unwrap()
            .clone()
            .into_iter()
            .collect()
    }

    /// Find the control sender associated with a device UUID, if present.
    fn sender_for_uuid(&self, uuid: [u8; 6]) -> Option<mpsc::Sender<DeviceTaskMessage>> {
        let guard = self.device_list.lock();
        guard
            .iter()
            .find(|(d, _tx)| d.uuid == uuid)
            .map(|(_d, tx)| tx.clone())
    }
}

#[tonic::async_trait]
impl Service for Server {
    // Stream associated types (use concrete tokio_stream wrappers; no futures_core)
    type SubscribeDeviceListStream = ReceiverStream<Result<DeviceListReply, Status>>;
    type SubscribeAccessoryMapStream = ReceiverStream<Result<AccessoryMapReply, Status>>;
    type SubscribeEventsStream = ReceiverStream<Result<Event, Status>>;
    type SubscribeShotDelayStream = ReceiverStream<Result<GetShotDelayReply, Status>>;
    // -------------------- Device list --------------------
    async fn get_device_list(
        &self,
        _req: tonic::Request<DeviceListRequest>,
    ) -> Result<Response<DeviceListReply>, Status> {
        let snap = self.device_snapshot();
        Ok(Response::new(DeviceListReply {
            device_list: snap.into_iter().map(|d| d.into()).collect(),
        }))
    }

    async fn subscribe_device_list(
        &self,
        _req: tonic::Request<SubscribeDeviceListRequest>,
    ) -> Result<Response<Self::SubscribeDeviceListStream>, Status> {
        let (rx, _handle) = self.device_list_stream();
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    // -------------------- Accessories --------------------
    async fn get_accessory_map(
        &self,
        _req: tonic::Request<AccessoryMapRequest>,
    ) -> Result<Response<AccessoryMapReply>, Status> {
        Ok(Response::new(AccessoryMapReply::from(
            self.accessory_map_snapshot(),
        )))
    }

    async fn subscribe_accessory_map(
        &self,
        _req: tonic::Request<SubscribeAccessoryMapRequest>,
    ) -> Result<Response<Self::SubscribeAccessoryMapStream>, Status> {
        // Initial snapshot + broadcast stream that emits common::AccessoryMap
        let init = self.accessory_map_snapshot();
        let mut bcast = BroadcastStream::new(self.accessory_map_sender.subscribe());

        let (tx, rx) = mpsc::channel::<Result<AccessoryMapReply, Status>>(16);
        let _ = tx.try_send(Ok(AccessoryMapReply::from(init)));

        tokio::spawn(async move {
            while let Some(item) = bcast.next().await {
                match item {
                    Ok(map) => {
                        if tx.send(Ok(AccessoryMapReply::from(map))).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn update_accessory_info_map(
        &self,
        req: tonic::Request<AccessoryInfoMap>,
    ) -> Result<Response<EmptyReply>, Status> {
        // Convert proto → common map
        let incoming: common::accessory::AccessoryInfoMap = req.into_inner().into();

        // Persist to disk using odyssey_hub_common::config
        if let Err(e) = common::config::accessory_map_save_async(&incoming).await {
            return Err(Status::internal(format!(
                "failed to save accessory info map: {e}"
            )));
        }

        // Re-load (optional but keeps canonical formatting)
        let fresh_result = common::config::accessory_map_async().await;
        let fresh = if fresh_result.is_corrupted() {
            tracing::warn!("Saved but file is corrupted on reload; using incoming");
            incoming
        } else {
            fresh_result.into_value()
        };

        // Notify watchers of info updates
        let _ = self.accessory_info_sender.send(fresh.clone());

        // Merge into (info, connected) map; keep connection flags
        if let Ok(mut g) = self.accessory_map.lock() {
            for (k, info) in fresh {
                let connected = g.get(&k).map(|(_, c)| *c).unwrap_or(false);
                g.insert(k, (info, connected));
            }
        }
        // Broadcast full status map so UI refreshes
        let _ = self
            .accessory_map_sender
            .send(self.accessory_map_snapshot());

        Ok(Response::new(EmptyReply {}))
    }

    // -------------------- Shot delay --------------------
    async fn get_shot_delay(
        &self,
        req: tonic::Request<Device>,
    ) -> Result<Response<GetShotDelayReply>, Status> {
        let mut uuid = [0u8; 6];
        let uuid_vec = req.into_inner().uuid;
        if uuid_vec.len() == 6 {
            uuid.copy_from_slice(&uuid_vec);
        } else if uuid_vec.len() > 0 {
            let len = std::cmp::min(6, uuid_vec.len());
            uuid[..len].copy_from_slice(&uuid_vec[..len]);
        }
        let cur = self.get_or_default_shot_delay(uuid).await as u32;
        Ok(Response::new(GetShotDelayReply { delay_ms: cur }))
    }

    async fn subscribe_shot_delay(
        &self,
        req: tonic::Request<SubscribeShotDelayRequest>,
    ) -> Result<Response<Self::SubscribeShotDelayStream>, Status> {
        let uuid_vec = req
            .into_inner()
            .device
            .ok_or_else(|| Status::invalid_argument("device missing"))?
            .uuid;
        let mut uuid = [0u8; 6];
        if uuid_vec.len() == 6 {
            uuid.copy_from_slice(&uuid_vec);
        } else if uuid_vec.len() > 0 {
            let len = std::cmp::min(6, uuid_vec.len());
            uuid[..len].copy_from_slice(&uuid_vec[..len]);
        }

        let mut rx = self.subscribe_shot_delay_watch(uuid).await;
        let (tx, out) = mpsc::channel::<Result<GetShotDelayReply, Status>>(16);

        // initial
        let _ = tx.try_send(Ok(GetShotDelayReply {
            delay_ms: *rx.borrow(),
        }));
        tokio::spawn(async move {
            while rx.changed().await.is_ok() {
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
        _req: tonic::Request<EmptyRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        // Send message to desktop app to bring window to front
        if let Err(e) = self.app_message_sender.send(crate::Message::BringToFront) {
            tracing::error!("Failed to send BringToFront message: {}", e);
            return Err(Status::internal("Failed to bring window to front"));
        }
        Ok(Response::new(EmptyReply {}))
    }

    async fn set_shot_delay(
        &self,
        req: tonic::Request<SetShotDelayRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let SetShotDelayRequest { device, delay_ms } = req.into_inner();
        let uuid_vec = device
            .ok_or_else(|| Status::invalid_argument("device missing"))?
            .uuid;
        let mut uuid = [0u8; 6];
        if uuid_vec.len() == 6 {
            uuid.copy_from_slice(&uuid_vec);
        } else if uuid_vec.len() > 0 {
            let len = std::cmp::min(6, uuid_vec.len());
            uuid[..len].copy_from_slice(&uuid_vec[..len]);
        }
        let ms: u16 = delay_ms as u16;

        // Apply live in-server state & notify watchers
        self.set_shot_delay_live(uuid, ms).await;

        // If a device task is running for this UUID, also send it a live update
        if let Some(tx) = self.sender_for_uuid(uuid) {
            match tx.try_send(DeviceTaskMessage::SetShotDelay(ms)) {
                Ok(()) => {}
                Err(_) => tracing::warn!(
                    "Device task not available to receive shot delay for {:02x?}",
                    uuid
                ),
            }
        }

        Ok(Response::new(EmptyReply {}))
    }

    async fn reset_shot_delay(
        &self,
        req: tonic::Request<Device>,
    ) -> Result<Response<ResetShotDelayReply>, Status> {
        let uuid_vec = req.into_inner().uuid;
        let mut uuid = [0u8; 6];
        if uuid_vec.len() == 6 {
            uuid.copy_from_slice(&uuid_vec);
        } else if uuid_vec.len() > 0 {
            let len = std::cmp::min(6, uuid_vec.len());
            uuid[..len].copy_from_slice(&uuid_vec[..len]);
        }
        // Policy: default 0ms
        let default_ms: u16 = 0;

        // Apply live reset in-server state & notify watchers
        self.set_shot_delay_live(uuid, default_ms).await;

        // If a device task is running for this UUID, send a live update to reset it as well
        if let Some(tx) = self.sender_for_uuid(uuid) {
            match tx.try_send(DeviceTaskMessage::SetShotDelay(default_ms)) {
                Ok(()) => {}
                Err(_) => tracing::warn!(
                    "Device task not available to receive shot delay reset for {:02x?}",
                    uuid
                ),
            }
        }

        Ok(Response::new(ResetShotDelayReply {
            delay_ms: default_ms as u32,
        }))
    }

    async fn save_shot_delay(
        &self,
        req: tonic::Request<Device>,
    ) -> Result<Response<EmptyReply>, Status> {
        let uuid_vec = req.into_inner().uuid;
        let mut uuid = [0u8; 6];
        if uuid_vec.len() == 6 {
            uuid.copy_from_slice(&uuid_vec);
        } else if uuid_vec.len() > 0 {
            let len = std::cmp::min(6, uuid_vec.len());
            uuid[..len].copy_from_slice(&uuid_vec[..len]);
        }
        let cur = self.get_or_default_shot_delay(uuid).await;
        self.persist_shot_delay(uuid, cur).await?;
        Ok(Response::new(EmptyReply {}))
    }

    // -------------------- Screens --------------------
    async fn get_screen_info_by_id(
        &self,
        req: tonic::Request<ScreenInfoByIdRequest>,
    ) -> Result<Response<ScreenInfoReply>, Status> {
        let id = req.into_inner().id as u8;
        let sc = self.screen_calibrations.load();
        let mut reply: Option<ScreenInfoReply> = None;
        for (sid, cal) in sc.iter() {
            if *sid == id {
                // convert bounds
                let b = cal.bounds();
                reply = Some(ScreenInfoReply {
                    id: id as u32,
                    bounds: Some(ScreenBounds {
                        tl: Some(Vector2::from(b[0].coords).into()),
                        tr: Some(Vector2::from(b[1].coords).into()),
                        bl: Some(Vector2::from(b[2].coords).into()),
                        br: Some(Vector2::from(b[3].coords).into()),
                    }),
                });
                break;
            }
        }
        match reply {
            Some(r) => Ok(Response::new(r)),
            None => Err(Status::not_found("screen id")),
        }
    }

    // -------------------- Events passthrough --------------------
    async fn subscribe_events(
        &self,
        _req: tonic::Request<SubscribeEventsRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        let mut rx = self.event_sender.subscribe();
        let (tx, out) = mpsc::channel::<Result<Event, Status>>(128);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let pb = Event::from(ev);
                        if tx.send(Ok(pb)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(out)))
    }

    // -------------------- Vendor / zero stubs --------------------
    async fn write_vendor(
        &self,
        req: tonic::Request<WriteVendorRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let WriteVendorRequest { device, tag, data } = req.into_inner();
        let dev: common::device::Device = device
            .ok_or_else(|| Status::invalid_argument("device missing"))?
            .into();

        // Find the per-device sender and forward as DeviceTaskMessage::WriteVendor
        if let Some(tx) = self.sender_for_uuid(dev.uuid) {
            // tag is u32 in proto; device tasks expect u8 vendor tag
            let tag_u8 = tag as u8;
            match tx.try_send(DeviceTaskMessage::WriteVendor(tag_u8, data)) {
                Ok(()) => Ok(Response::new(EmptyReply {})),
                Err(_) => Err(Status::unavailable("device task not available")),
            }
        } else {
            Err(Status::not_found("device"))
        }
    }

    async fn read_register(
        &self,
        req: tonic::Request<ReadRegisterRequest>,
    ) -> Result<Response<ReadRegisterReply>, Status> {
        let inner = req.into_inner();
        let dev: common::device::Device = inner
            .device
            .ok_or_else(|| Status::invalid_argument("device missing"))?
            .into();
        if let Some(tx) = self.sender_for_uuid(dev.uuid) {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            tx.send(DeviceTaskMessage::ReadRegister {
                port: inner.port as u8,
                bank: inner.bank as u8,
                address: inner.address as u8,
                reply: reply_tx,
            })
            .await
            .map_err(|_| Status::unavailable("device task not available"))?;
            match reply_rx.await {
                Ok(Ok(data)) => Ok(Response::new(ReadRegisterReply { data: data as u32 })),
                Ok(Err(e)) => Err(Status::internal(e)),
                Err(_) => Err(Status::internal("device task dropped reply")),
            }
        } else {
            Err(Status::not_found("device"))
        }
    }

    async fn write_register(
        &self,
        req: tonic::Request<WriteRegisterRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let inner = req.into_inner();
        let dev: common::device::Device = inner
            .device
            .ok_or_else(|| Status::invalid_argument("device missing"))?
            .into();
        if let Some(tx) = self.sender_for_uuid(dev.uuid) {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            tx.send(DeviceTaskMessage::WriteRegister {
                port: inner.port as u8,
                bank: inner.bank as u8,
                address: inner.address as u8,
                data: inner.data as u8,
                reply: reply_tx,
            })
            .await
            .map_err(|_| Status::unavailable("device task not available"))?;
            match reply_rx.await {
                Ok(Ok(())) => Ok(Response::new(EmptyReply {})),
                Ok(Err(e)) => Err(Status::internal(e)),
                Err(_) => Err(Status::internal("device task dropped reply")),
            }
        } else {
            Err(Status::not_found("device"))
        }
    }

    async fn flash_settings(
        &self,
        req: tonic::Request<FlashSettingsRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let dev: common::device::Device = req
            .into_inner()
            .device
            .ok_or_else(|| Status::invalid_argument("device missing"))?
            .into();
        if let Some(tx) = self.sender_for_uuid(dev.uuid) {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            tx.send(DeviceTaskMessage::FlashSettings { reply: reply_tx })
                .await
                .map_err(|_| Status::unavailable("device task not available"))?;
            match reply_rx.await {
                Ok(Ok(())) => Ok(Response::new(EmptyReply {})),
                Ok(Err(e)) => Err(Status::internal(e)),
                Err(_) => Err(Status::internal("device task dropped reply")),
            }
        } else {
            Err(Status::not_found("device"))
        }
    }

    async fn clear_zero(
        &self,
        req: tonic::Request<Device>,
    ) -> Result<Response<EmptyReply>, Status> {
        let dev: common::device::Device = req.into_inner().into();

        if let Some(tx) = self.sender_for_uuid(dev.uuid) {
            match tx.try_send(DeviceTaskMessage::ClearZero) {
                Ok(()) => Ok(Response::new(EmptyReply {})),
                Err(_) => Err(Status::unavailable("device task not available")),
            }
        } else {
            Err(Status::not_found("device"))
        }
    }

    async fn reset_zero(
        &self,
        req: tonic::Request<Device>,
    ) -> Result<Response<EmptyReply>, Status> {
        let dev: common::device::Device = req.into_inner().into();

        if let Some(tx) = self.sender_for_uuid(dev.uuid) {
            match tx.try_send(DeviceTaskMessage::ResetZero) {
                Ok(()) => Ok(Response::new(EmptyReply {})),
                Err(_) => Err(Status::unavailable("device task not available")),
            }
        } else {
            Err(Status::not_found("device"))
        }
    }

    async fn save_zero(&self, req: tonic::Request<Device>) -> Result<Response<EmptyReply>, Status> {
        let dev: common::device::Device = req.into_inner().into();

        if let Some(tx) = self.sender_for_uuid(dev.uuid) {
            match tx.try_send(DeviceTaskMessage::SaveZero) {
                Ok(()) => Ok(Response::new(EmptyReply {})),
                Err(_) => Err(Status::unavailable("device task not available")),
            }
        } else {
            Err(Status::not_found("device"))
        }
    }

    async fn zero(&self, req: tonic::Request<ZeroRequest>) -> Result<Response<EmptyReply>, Status> {
        let ZeroRequest {
            device,
            translation,
            target,
        } = req.into_inner();

        let dev: common::device::Device = device
            .ok_or_else(|| Status::invalid_argument("device missing"))?
            .into();

        let translation_vec: nalgebra::Vector3<f32> = translation
            .ok_or_else(|| Status::invalid_argument("translation missing"))?
            .into();
        let target_vec: nalgebra::Vector2<f32> = target
            .ok_or_else(|| Status::invalid_argument("target missing"))?
            .into();

        // Convert to types expected by device task
        let trans = nalgebra::Translation3::from(translation_vec);
        let point = nalgebra::Point2::from(target_vec);

        if let Some(tx) = self.sender_for_uuid(dev.uuid) {
            match tx.try_send(DeviceTaskMessage::Zero(trans, point)) {
                Ok(()) => Ok(Response::new(EmptyReply {})),
                Err(_) => Err(Status::unavailable("device task not available")),
            }
        } else {
            Err(Status::not_found("device"))
        }
    }
}
