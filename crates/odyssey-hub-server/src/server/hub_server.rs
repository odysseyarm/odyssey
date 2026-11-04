use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use arc_swap::ArcSwap;
use odyssey_hub_common as common;
use odyssey_hub_common::accessory::{AccessoryInfoMap, AccessoryMap};
use odyssey_hub_server_interface::{DeviceListReply, Dongle, DongleListReply, FirmwareVersion};
use tokio::sync::{RwLock, broadcast, mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::Status;

use super::rpc_error::RpcError;
use super::task_sender::TaskSender;
use crate::device_list::DeviceList;
use crate::device_tasks::{DeviceTaskMessage, DongleInfo, DongleTaskMessage};
use crate::dongle_list::DongleList;
use crate::screen_watch::Calibrations;

/// Capacity of the channel that feeds one streaming RPC; a client that falls
/// further behind makes the producing task wait.
pub(super) const SUBSCRIPTION_BUFFER: usize = 16;

/// The gRPC service and the hub state it shares with the device tasks.
#[derive(Clone)]
pub(crate) struct HubServer {
    /// Live devices, kept current by the message forwarder.
    pub device_list: DeviceList,

    /// Pub/Sub of low-level events used by existing pipeline.
    pub event_sender: broadcast::Sender<common::events::Event>,

    /// Notifies subscribers when device list changes (Connect/Disconnect)
    pub device_list_change_sender: broadcast::Sender<()>,

    pub screen_calibrations: Arc<ArcSwap<Calibrations>>,

    /// Shot delay store (uuid→delay), plus per-uuid watchers for SubscribeShotDelay.
    pub device_shot_delays: Arc<RwLock<HashMap<[u8; 6], u16>>>,
    pub shot_delay_watch: Arc<RwLock<HashMap<[u8; 6], watch::Sender<u32>>>>,

    /// Accessory info/state
    pub accessory_map: Arc<Mutex<AccessoryMap>>,
    pub accessory_map_sender: broadcast::Sender<AccessoryMap>,

    /// Live dongle list (separate from device_list).
    pub dongle_list: DongleList,

    /// Notifies subscribers when dongle list changes
    pub dongle_list_change_sender: broadcast::Sender<()>,

    /// Message sender to communicate with the desktop app
    pub app_message_sender: mpsc::UnboundedSender<crate::Message>,
    pub accessory_info_sender: watch::Sender<AccessoryInfoMap>, // incoming updates
}

impl HubServer {
    /// The command channel of a listed device's task.
    pub(super) fn device_task(
        &self,
        rpc: &'static str,
        uuid: [u8; 6],
    ) -> Result<TaskSender<DeviceTaskMessage>, RpcError> {
        let Some(sender) = self.device_list.task_sender(uuid) else {
            tracing::warn!("{rpc}: device {uuid:02x?} not found in device list");
            return Err(RpcError::NotFound { task: "device" });
        };
        Ok(TaskSender::new("device", sender))
    }

    /// The command channel of a listed dongle's task.
    pub(super) fn dongle_task(&self, id: &str) -> Result<TaskSender<DongleTaskMessage>, RpcError> {
        let sender = self
            .dongle_list
            .task_sender(id)
            .ok_or(RpcError::NotFound { task: "dongle" })?;
        Ok(TaskSender::new("dongle", sender))
    }

    pub(super) fn device_list_reply(&self) -> DeviceListReply {
        device_list_reply_from(&self.device_list)
    }

    /// Sends the device list now and again after every change.
    pub(super) fn device_list_updates(&self) -> ReceiverStream<Result<DeviceListReply, Status>> {
        let device_list = self.device_list.clone();
        stream_replies(&self.device_list_change_sender, "device list", move || {
            device_list_reply_from(&device_list)
        })
    }

    /// Sends the dongle list now and again after every change.
    pub(super) fn dongle_list_updates(&self) -> ReceiverStream<Result<DongleListReply, Status>> {
        let dongle_list = self.dongle_list.clone();
        stream_replies(&self.dongle_list_change_sender, "dongle list", move || {
            dongle_list_reply_from(&dongle_list.snapshot())
        })
    }

    // -------------------- Shot delay helpers --------------------
    pub(super) async fn shot_delay_or_default(&self, uuid: [u8; 6]) -> u16 {
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
        let mut watchers = self.shot_delay_watch.write().await;
        if let Some(tx) = watchers.get(&uuid) {
            let _ = tx.send(u32::from(ms));
        } else {
            let (tx, _rx) = watch::channel(u32::from(ms));
            let _ = tx.send(u32::from(ms));
            watchers.insert(uuid, tx);
        }
    }

    pub(super) async fn subscribe_shot_delay_watch(&self, uuid: [u8; 6]) -> watch::Receiver<u32> {
        let current = u32::from(self.shot_delay_or_default(uuid).await);
        let mut watchers = self.shot_delay_watch.write().await;
        if let Some(tx) = watchers.get(&uuid) {
            tx.subscribe()
        } else {
            let (tx, rx) = watch::channel(current);
            watchers.insert(uuid, tx);
            rx
        }
    }

    /// Tells the running device task, if any, about a shot delay change. `change`
    /// names the change in the warning when the task cannot take it.
    pub(super) fn push_shot_delay_to_task(&self, uuid: [u8; 6], ms: u16, change: &str) {
        let Some(sender) = self.device_list.task_sender(uuid) else {
            return;
        };
        if sender
            .try_send(DeviceTaskMessage::SetShotDelay(ms))
            .is_err()
        {
            tracing::warn!("Device task not available to receive {change} for {uuid:02x?}");
        }
    }

    // -------------------- Accessory map helpers --------------------
    fn lock_accessory_map(&self) -> MutexGuard<'_, AccessoryMap> {
        // The map is only ever replaced entry by entry, so a panic elsewhere
        // cannot leave it half updated; carry on with what is there.
        self.accessory_map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn accessory_map_snapshot(&self) -> AccessoryMap {
        self.lock_accessory_map().clone()
    }

    /// Takes new accessory info into the shared map, keeping each accessory's
    /// connection flag.
    pub(super) fn merge_accessory_info(&self, info: AccessoryInfoMap) {
        let mut map = self.lock_accessory_map();
        for (id, info) in info {
            let connected = map.get(&id).is_some_and(|(_, connected)| *connected);
            map.insert(id, (info, connected));
        }
    }
}

/// Feeds a client the reply built by `reply`, once right away and again after
/// every notification on `changes`.
///
/// The producing task is detached on purpose: it ends by itself when the client
/// goes away or the notification channel closes, and has no result to report.
fn stream_replies<Reply: Send + 'static>(
    changes: &broadcast::Sender<()>,
    list_name: &'static str,
    reply: impl Fn() -> Reply + Send + 'static,
) -> ReceiverStream<Result<Reply, Status>> {
    let mut change_rx = changes.subscribe();
    let (tx, rx) = mpsc::channel(SUBSCRIPTION_BUFFER);

    let _ = tx.try_send(Ok(reply()));

    tokio::spawn(async move {
        loop {
            match change_rx.recv().await {
                Ok(()) => {}
                // Lagging only means notifications were dropped while this
                // task was busy; the next reply is rebuilt from the live list
                // and so still authoritative. Coalesce them rather than
                // ending the subscription and leaving the client frozen on a
                // stale list.
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("{list_name} subscriber lagged {n} notifications");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
            if tx.send(Ok(reply())).await.is_err() {
                break; // client gone
            }
        }
    });
    ReceiverStream::new(rx)
}

fn device_list_reply_from(device_list: &DeviceList) -> DeviceListReply {
    DeviceListReply {
        device_list: device_list.snapshot().into_iter().map(Into::into).collect(),
    }
}

/// The version as the protocol carries it: all zeros means the device never said.
fn reported_version(version: [u16; 3]) -> Option<FirmwareVersion> {
    if version == [0, 0, 0] {
        return None;
    }
    Some(FirmwareVersion {
        major: u32::from(version[0]),
        minor: u32::from(version[1]),
        patch: u32::from(version[2]),
    })
}

fn dongle_list_reply_from(dongles: &[DongleInfo]) -> DongleListReply {
    DongleListReply {
        dongle_list: dongles
            .iter()
            .map(|d| Dongle {
                id: d.id.clone(),
                firmware_version: reported_version(d.firmware_version),
                connected_devices: d
                    .connected_devices
                    .iter()
                    .map(|addr| addr.to_vec())
                    .collect(),
                protocol_version: reported_version(d.protocol_version),
                control_protocol_version: reported_version(d.control_protocol_version),
                bonded_devices: d.bonded_devices.iter().map(|addr| addr.to_vec()).collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreported_versions_are_left_out_of_the_dongle_reply() {
        let dongle = DongleInfo {
            id: "serial".to_owned(),
            protocol_version: [0, 0, 0],
            control_protocol_version: [1, 2, 3],
            firmware_version: [0, 4, 0],
            connected_devices: vec![[1; 6]],
            bonded_devices: Vec::new(),
        };

        let reply = dongle_list_reply_from(&[dongle]);

        let reply = &reply.dongle_list[0];
        assert_eq!(reply.protocol_version, None);
        assert_eq!(
            reply.control_protocol_version,
            Some(FirmwareVersion {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(reply.firmware_version.as_ref().map(|v| v.minor), Some(4));
        assert_eq!(reply.connected_devices, vec![vec![1; 6]]);
    }
}
