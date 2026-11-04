use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use nalgebra::Isometry3;
use odyssey_common::ScreenCalibration;
use odyssey_hub_common as common;
use odyssey_hub_server_interface::{DeviceListReply, Dongle, DongleListReply, FirmwareVersion};
use parking_lot::Mutex as ParkingMutex;
use tokio::sync::{RwLock, broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tonic::Status;

use crate::device_tasks::{DeviceTaskMessage, DongleInfo, DongleTaskMessage};

/// gRPC service state
pub(crate) struct HubServer {
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
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (odyssey_common::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
    #[expect(unused)]
    pub device_offsets: Arc<tokio::sync::Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,

    /// Shot delay store (uuid→delay), plus per-uuid watchers for SubscribeShotDelay.
    pub device_shot_delays: Arc<RwLock<HashMap<[u8; 6], u16>>>, // seeded by lib.rs at startup
    pub shot_delay_watch: Arc<RwLock<HashMap<[u8; 6], watch::Sender<u32>>>>,

    /// Accessory info/state
    pub accessory_map:
        Arc<std::sync::Mutex<HashMap<[u8; 6], (common::accessory::AccessoryInfo, bool)>>>,
    pub accessory_map_sender: broadcast::Sender<common::accessory::AccessoryMap>,

    /// Live dongle list (separate from device_list).
    pub dongle_list: Arc<ParkingMutex<Vec<(DongleInfo, mpsc::Sender<DongleTaskMessage>)>>>,

    /// Notifies subscribers when dongle list changes
    pub dongle_list_change_sender: broadcast::Sender<()>,

    /// Message sender to communicate with the desktop app
    pub app_message_sender: mpsc::UnboundedSender<crate::Message>,
    pub accessory_info_sender: watch::Sender<HashMap<[u8; 6], common::accessory::AccessoryInfo>>, // incoming updates
}

impl HubServer {
    // -------------------- Device list helpers --------------------
    pub(super) fn device_snapshot(&self) -> Vec<common::device::Device> {
        self.device_list
            .lock()
            .iter()
            .map(|(d, _tx)| d.clone())
            .collect()
    }

    pub(super) fn device_list_stream(
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
            device_list: snapshot.into_iter().map(Into::into).collect(),
        }));

        // Clone only what the task needs so the future is 'static
        let device_list = self.device_list.clone();
        let handle = tokio::spawn(async move {
            loop {
                let changed = match change_rx.recv().await {
                    Ok(()) => true,
                    // Lagging only means notifications were dropped while this
                    // task was busy; the next snapshot is still authoritative,
                    // so coalesce them rather than ending the subscription and
                    // leaving the client frozen on a stale device list.
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!("device list subscriber lagged {n} notifications");
                        true
                    }
                    Err(broadcast::error::RecvError::Closed) => false,
                };
                if !changed {
                    break;
                }
                // Build a fresh snapshot and push
                let snap = {
                    let guard = device_list.lock();
                    guard.iter().map(|(d, _tx)| d.clone()).collect::<Vec<_>>()
                };
                if tx
                    .send(Ok(DeviceListReply {
                        device_list: snap.into_iter().map(Into::into).collect(),
                    }))
                    .await
                    .is_err()
                {
                    break; // client gone
                }
            }
        });
        (rx, handle)
    }

    // -------------------- Dongle list helpers --------------------
    pub(super) fn dongle_snapshot(&self) -> Vec<DongleInfo> {
        self.dongle_list
            .lock()
            .iter()
            .map(|(d, _)| d.clone())
            .collect()
    }

    pub(super) fn dongle_list_stream(
        &self,
    ) -> (
        mpsc::Receiver<Result<DongleListReply, Status>>,
        JoinHandle<()>,
    ) {
        let mut change_rx = self.dongle_list_change_sender.subscribe();
        let (tx, rx) = mpsc::channel::<Result<DongleListReply, Status>>(16);

        let snapshot = self.dongle_snapshot();
        let _ = tx.try_send(Ok(dongle_list_reply_from(&snapshot)));

        let dongle_list = self.dongle_list.clone();
        let handle = tokio::spawn(async move {
            loop {
                let changed = match change_rx.recv().await {
                    Ok(()) => true,
                    // A dropped notification costs nothing: the snapshot below
                    // is rebuilt from the live list either way.
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!("dongle list subscriber lagged {n} notifications");
                        true
                    }
                    Err(broadcast::error::RecvError::Closed) => false,
                };
                if !changed {
                    break;
                }
                let snap = {
                    let guard = dongle_list.lock();
                    guard.iter().map(|(d, _)| d.clone()).collect::<Vec<_>>()
                };
                if tx.send(Ok(dongle_list_reply_from(&snap))).await.is_err() {
                    break;
                }
            }
        });
        (rx, handle)
    }

    pub(super) fn dongle_sender_for_id(&self, id: &str) -> Option<mpsc::Sender<DongleTaskMessage>> {
        let guard = self.dongle_list.lock();
        guard
            .iter()
            .find(|(d, _)| d.id == id)
            .map(|(_, tx)| tx.clone())
    }

    // -------------------- Shot delay helpers --------------------
    pub(super) async fn get_or_default_shot_delay(&self, uuid: [u8; 6]) -> u16 {
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

    pub(super) async fn set_shot_delay_live(&self, uuid: [u8; 6], ms: u16) {
        self.device_shot_delays.write().await.insert(uuid, ms);
        let mut g = self.shot_delay_watch.write().await;
        if let Some(tx) = g.get(&uuid) {
            let _ = tx.send(u32::from(ms));
        } else {
            let (tx, _rx) = watch::channel(u32::from(ms));
            let _ = tx.send(u32::from(ms));
            g.insert(uuid, tx);
        }
    }

    pub(super) async fn subscribe_shot_delay_watch(&self, uuid: [u8; 6]) -> watch::Receiver<u32> {
        let cur = u32::from(self.get_or_default_shot_delay(uuid).await);
        let mut g = self.shot_delay_watch.write().await;
        if let Some(tx) = g.get(&uuid) {
            tx.subscribe()
        } else {
            let (tx, rx) = watch::channel(cur);
            g.insert(uuid, tx);
            rx
        }
    }

    pub(super) async fn persist_shot_delay(&self, uuid: [u8; 6], ms: u16) -> Result<(), Status> {
        common::config::device_shot_delay_save_async(uuid, ms)
            .await
            .map_err(|e| Status::internal(format!("failed to save delay: {e}")))
    }

    // -------------------- Accessory map helpers --------------------
    pub(super) fn accessory_map_snapshot(&self) -> common::accessory::AccessoryMap {
        // Convert HashMap<[u8;6], (info, connected)> to AccessoryMap type alias
        self.accessory_map
            .lock()
            .unwrap()
            .clone()
            .into_iter()
            .collect()
    }

    /// Find the control sender associated with a device UUID, if present.
    pub(super) fn sender_for_uuid(&self, uuid: [u8; 6]) -> Option<mpsc::Sender<DeviceTaskMessage>> {
        let guard = self.device_list.lock();
        guard
            .iter()
            .find(|(d, _tx)| d.uuid == uuid)
            .map(|(_d, tx)| tx.clone())
    }
}

fn dongle_list_reply_from(dongles: &[DongleInfo]) -> DongleListReply {
    DongleListReply {
        dongle_list: dongles
            .iter()
            .map(|d| Dongle {
                id: d.id.clone(),
                firmware_version: if d.firmware_version == [0, 0, 0] {
                    None
                } else {
                    Some(FirmwareVersion {
                        major: u32::from(d.firmware_version[0]),
                        minor: u32::from(d.firmware_version[1]),
                        patch: u32::from(d.firmware_version[2]),
                    })
                },
                connected_devices: d
                    .connected_devices
                    .iter()
                    .map(|addr| addr.to_vec())
                    .collect(),
                protocol_version: if d.protocol_version == [0, 0, 0] {
                    None
                } else {
                    Some(FirmwareVersion {
                        major: u32::from(d.protocol_version[0]),
                        minor: u32::from(d.protocol_version[1]),
                        patch: u32::from(d.protocol_version[2]),
                    })
                },
                control_protocol_version: if d.control_protocol_version == [0, 0, 0] {
                    None
                } else {
                    Some(FirmwareVersion {
                        major: u32::from(d.control_protocol_version[0]),
                        minor: u32::from(d.control_protocol_version[1]),
                        patch: u32::from(d.control_protocol_version[2]),
                    })
                },
                bonded_devices: d.bonded_devices.iter().map(|addr| addr.to_vec()).collect(),
            })
            .collect(),
    }
}
