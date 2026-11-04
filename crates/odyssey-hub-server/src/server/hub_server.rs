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
        let mut watchers = self.shot_delay_watch.write().await;
        self.device_shot_delays.write().await.insert(uuid, ms);
        if let Some(tx) = watchers.get(&uuid) {
            tx.send_replace(u32::from(ms));
        } else {
            let (tx, _rx) = watch::channel(u32::from(ms));
            watchers.insert(uuid, tx);
        }
    }

    pub(super) async fn subscribe_shot_delay_watch(&self, uuid: [u8; 6]) -> watch::Receiver<u32> {
        let mut watchers = self.shot_delay_watch.write().await;
        if let Some(tx) = watchers.get(&uuid) {
            tx.subscribe()
        } else {
            let current = u32::from(self.shot_delay_or_default(uuid).await);
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

    /// Takes new accessory info into the shared map. A changed accessory type
    /// requires a new notification subscription and cannot retain a connected flag.
    pub(super) fn merge_accessory_info(&self, info: AccessoryInfoMap) {
        let mut map = self.lock_accessory_map();
        map.retain(|id, _| info.contains_key(id));
        for (id, info) in info {
            let connected = map
                .get(&id)
                .is_some_and(|(previous, connected)| previous.ty == info.ty && *connected);
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
            let change = tokio::select! {
                () = tx.closed() => break,
                change = change_rx.recv() => change,
            };
            match change {
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

    fn server() -> HubServer {
        HubServer {
            device_list: Default::default(),
            event_sender: broadcast::channel(1).0,
            device_list_change_sender: broadcast::channel(1).0,
            screen_calibrations: Arc::new(ArcSwap::from_pointee(Calibrations::default())),
            device_shot_delays: Default::default(),
            shot_delay_watch: Default::default(),
            accessory_map: Default::default(),
            accessory_map_sender: broadcast::channel(1).0,
            dongle_list: Default::default(),
            dongle_list_change_sender: broadcast::channel(1).0,
            app_message_sender: mpsc::unbounded_channel().0,
            accessory_info_sender: watch::channel(AccessoryInfoMap::new()).0,
        }
    }

    #[tokio::test]
    async fn rpc_arguments_are_rejected_before_narrowing_or_mutating_state() {
        use odyssey_hub_server_interface::service_server::Service as _;
        use odyssey_hub_server_interface::{
            Device, ReadConfigRequest, ReadRegisterRequest, ScreenInfoByIdRequest,
            SetShotDelayRequest, WriteConfigRequest, WriteRegisterRequest, WriteVendorRequest,
        };
        use tonic::{Code, Request};

        let server = server();
        let device = || {
            Some(Device {
                uuid: vec![1; 6],
                ..Default::default()
            })
        };
        let error = server
            .set_shot_delay(Request::new(SetShotDelayRequest {
                device: device(),
                delay_ms: 65536,
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);
        assert!(server.device_shot_delays.read().await.is_empty());
        server
            .set_shot_delay(Request::new(SetShotDelayRequest {
                device: device(),
                delay_ms: u32::from(u16::MAX),
            }))
            .await
            .unwrap();
        assert_eq!(
            server.device_shot_delays.read().await.get(&[1; 6]),
            Some(&u16::MAX)
        );

        let error = server
            .get_screen_info_by_id(Request::new(ScreenInfoByIdRequest { id: 256 }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);

        let error = server
            .read_register(Request::new(ReadRegisterRequest {
                device: device(),
                port: 0,
                bank: 256,
                address: 0,
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);
        let error = server
            .write_register(Request::new(WriteRegisterRequest {
                device: device(),
                port: 2,
                bank: 0,
                address: 0,
                data: 0,
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);

        let error = server
            .read_config(Request::new(ReadConfigRequest {
                device: device(),
                kind: 256,
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);
        let error = server
            .write_config(Request::new(WriteConfigRequest {
                device: device(),
                kind: 0,
                value: 256,
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);

        let error = server
            .write_vendor(Request::new(WriteVendorRequest {
                device: device(),
                tag: 0x181,
                data: vec![],
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);
        let error = server
            .write_vendor(Request::new(WriteVendorRequest {
                device: device(),
                tag: 0x81,
                data: vec![0; 99],
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);

        let error = server
            .reset_zero(Request::new(Device {
                uuid: vec![1; 7],
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn shot_delay_retains_updates_between_subscribers() {
        let server = server();
        let uuid = [1; 6];
        server.set_shot_delay_live(uuid, 10).await;
        let subscription = server.subscribe_shot_delay_watch(uuid).await;
        assert_eq!(*subscription.borrow(), 10);
        drop(subscription);

        server.set_shot_delay_live(uuid, 20).await;
        let subscription = server.subscribe_shot_delay_watch(uuid).await;
        assert_eq!(*subscription.borrow(), 20);
    }

    #[tokio::test]
    async fn idle_list_subscription_releases_its_state_on_drop() {
        let (changes, _) = broadcast::channel(1);
        let (lifetime, mut released) = mpsc::unbounded_channel::<()>();
        let stream = stream_replies(&changes, "test", move || {
            let _ = &lifetime;
        });
        drop(stream);

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), released.recv())
            .await
            .expect("idle subscription must release its snapshot state");
        assert_eq!(result, None);
    }

    #[test]
    fn accessory_info_replacement_removes_absent_entries_and_keeps_connection_state() {
        use common::accessory::{AccessoryInfo, AccessoryType};

        let server = server();
        let old = AccessoryInfo::with_defaults("old", AccessoryType::DryFireMag);
        let updated = AccessoryInfo::with_defaults("updated", AccessoryType::MantisX);
        *server.lock_accessory_map() =
            [([1; 6], (old.clone(), true)), ([2; 6], (old, false))].into();

        server.merge_accessory_info([([1; 6], updated.clone())].into());
        assert_eq!(
            server.accessory_map_snapshot(),
            [([1; 6], (updated, false))].into()
        );
    }

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
