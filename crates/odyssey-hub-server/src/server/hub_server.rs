use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use odyssey_hub_common as common;
use odyssey_hub_common::accessory::{AccessoryInfoMap, AccessoryMap};
use odyssey_hub_server_interface::{DeviceListReply, Dongle, DongleListReply, FirmwareVersion};
use parking_lot::{Mutex, MutexGuard};
use tokio::sync::{RwLock, broadcast, mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::Status;

use super::rpc_error::RpcError;
use super::task_sender::TaskSender;
use crate::device_list::DeviceList;
use crate::device_tasks::{DeviceTaskMessage, DongleInfo, DongleTaskMessage};
use crate::dongle_list::DongleList;
use crate::screen_watch::Calibrations;

/// Capacity of the channel that feeds one streaming RPC. A client that falls further
/// behind makes the producing task wait.
pub(super) const SUBSCRIPTION_BUFFER: usize = 16;

/// The gRPC service and the hub state it shares with the device tasks.
#[derive(Clone)]
pub(crate) struct HubServer {
    /// Live devices, kept current by the message forwarder.
    pub device_list: DeviceList,

    /// Low-level device events for every subscriber.
    pub event_sender: broadcast::Sender<common::events::Event>,
    pub marker_demand: crate::accessories::MarkerDemand,

    /// Fires on every device list change (connect, disconnect, update).
    pub device_list_change_sender: broadcast::Sender<()>,

    pub screen_calibrations: Arc<ArcSwap<Calibrations>>,

    /// Shot delay per uuid, and the per-uuid watches behind `SubscribeShotDelay`.
    pub device_shot_delays: Arc<RwLock<HashMap<[u8; 6], u16>>>,
    pub shot_delay_watch: Arc<RwLock<HashMap<[u8; 6], watch::Sender<u32>>>>,

    /// Accessory state, and the broadcast of the whole map to subscribers.
    pub accessory_map: Arc<Mutex<AccessoryMap>>,
    pub accessory_map_sender: broadcast::Sender<AccessoryMap>,
    pub accessory_radios: crate::accessories::AccessoryRadios,
    pub accessory_config_lock: Arc<tokio::sync::Mutex<()>>,

    /// Live dongles, kept apart from `device_list`.
    pub dongle_list: DongleList,

    /// Fires on every dongle list change.
    pub dongle_list_change_sender: broadcast::Sender<()>,

    /// Messages to the desktop app.
    pub app_message_sender: mpsc::UnboundedSender<crate::Message>,
    /// Incoming accessory info, watched by the accessory manager.
    pub accessory_info_sender: watch::Sender<AccessoryInfoMap>,
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

    /// Sends the device list at once and again after every change.
    pub(super) fn device_list_updates(&self) -> ReceiverStream<Result<DeviceListReply, Status>> {
        let device_list = self.device_list.clone();
        stream_replies(&self.device_list_change_sender, "device list", move || {
            device_list_reply_from(&device_list)
        })
    }

    /// Sends the dongle list at once and again after every change.
    pub(super) fn dongle_list_updates(&self) -> ReceiverStream<Result<DongleListReply, Status>> {
        let dongle_list = self.dongle_list.clone();
        stream_replies(&self.dongle_list_change_sender, "dongle list", move || {
            dongle_list_reply_from(&dongle_list.snapshot())
        })
    }

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

    fn lock_accessory_map(&self) -> MutexGuard<'_, AccessoryMap> {
        self.accessory_map.lock()
    }

    pub(super) fn accessory_map_snapshot(&self) -> AccessoryMap {
        self.lock_accessory_map().clone()
    }

    /// Takes new accessory info into the shared map. A changed accessory type needs a
    /// new notification subscription, so it does not keep its connected flag.
    pub(super) fn merge_accessory_info(&self, info: AccessoryInfoMap) {
        let mut map = self.lock_accessory_map();
        map.retain(|id, _| info.contains_key(id));
        for (id, info) in info {
            let connected = map.get(&id).is_some_and(|(previous, connected)| {
                previous.ty == info.ty && previous.radio == info.radio && *connected
            });
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
                // Lagging means notifications were dropped while this task was busy.
                // The next reply is rebuilt from the live list and is current anyway.
                // Ending the subscription would freeze the client on a stale list.
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("{list_name} subscriber lagged {n} notifications");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
            if tx.send(Ok(reply())).await.is_err() {
                break;
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
fn reported_version<T>(version: [T; 3]) -> Option<FirmwareVersion>
where
    T: Copy + Default + PartialEq + Into<u32>,
{
    if version == [T::default(); 3] {
        return None;
    }
    Some(FirmwareVersion {
        major: version[0].into(),
        minor: version[1].into(),
        patch: version[2].into(),
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
            device_list: DeviceList::default(),
            event_sender: broadcast::channel(1).0,
            marker_demand: crate::accessories::MarkerDemand::default(),
            device_list_change_sender: broadcast::channel(1).0,
            screen_calibrations: Arc::new(ArcSwap::from_pointee(Calibrations::default())),
            device_shot_delays: Arc::default(),
            shot_delay_watch: Arc::default(),
            accessory_map: Arc::default(),
            accessory_map_sender: broadcast::channel(1).0,
            accessory_radios: crate::accessories::AccessoryRadios::new().0,
            accessory_config_lock: Arc::default(),
            dongle_list: DongleList::default(),
            dongle_list_change_sender: broadcast::channel(1).0,
            app_message_sender: mpsc::unbounded_channel().0,
            accessory_info_sender: watch::channel(AccessoryInfoMap::new()).0,
        }
    }
    #[tokio::test]
    async fn radio_rpc_stream_preserves_unknown_capacity_fixed_route_errors_and_clone_connection() {
        use common::accessory::{
            AccessoryConnectionInfo, AccessoryConnectionState, AccessoryRadio, AccessoryRadioInfo,
        };
        use odyssey_hub_server_interface::service_client::ServiceClient;
        use odyssey_hub_server_interface::service_server::ServiceServer;
        use odyssey_hub_server_interface::{EmptyRequest, EnrollAccessoryRequest};
        use tokio_stream::wrappers::TcpListenerStream;
        use tokio_util::sync::CancellationToken;

        let server = server();
        let snapshots = server.accessory_radios.snapshots.clone();
        let mut native = AccessoryRadioInfo {
            radio: AccessoryRadio::Native {
                id: "00:11:22:33:44:55".into(),
            },
            name: "native".into(),
            is_default: true,
            available: true,
            accessory_supported: true,
            connected_accessories: 8,
            connecting_accessories: 1,
            connected_trackers: 0,
            capacity: None,
            accessory_capacity: None,
            connections: Vec::new(),
            nearby: Vec::new(),
            error: None,
        };
        snapshots.send_replace(vec![native.clone()]);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let cancel = CancellationToken::new();
        let shutdown = cancel.clone();
        let task = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(ServiceServer::new(server))
                .serve_with_incoming_shutdown(
                    TcpListenerStream::new(listener),
                    shutdown.cancelled(),
                )
                .await
                .unwrap();
        });
        let original = ServiceClient::connect(format!("http://{address}"))
            .await
            .unwrap();
        let mut client = original.clone();
        drop(original);
        let decoded: Vec<AccessoryRadioInfo> = client
            .get_accessory_radios(EmptyRequest {})
            .await
            .unwrap()
            .into_inner()
            .try_into()
            .unwrap();
        assert_eq!(decoded[0].connected_accessories, 8);
        assert_eq!(decoded[0].connecting_accessories, 1);
        assert_eq!(decoded[0].capacity, None);
        let mut stream = client
            .subscribe_accessory_radios(EmptyRequest {})
            .await
            .unwrap()
            .into_inner();
        stream.message().await.unwrap().unwrap();
        native.available = false;
        native.connected_accessories = 0;
        native.connecting_accessories = 0;
        native.error = Some("org.bluez.Error.NotReady: adapter powered off".into());
        native.connections.push(AccessoryConnectionInfo {
            address: [7; 6],
            state: AccessoryConnectionState::RadioUnavailable,
            error: Some("org.bluez.Error.NotReady: adapter powered off".into()),
        });
        snapshots.send_replace(vec![native.clone()]);
        let decoded: Vec<AccessoryRadioInfo> =
            tokio::time::timeout(std::time::Duration::from_secs(2), stream.message())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .try_into()
                .unwrap();
        assert_eq!(decoded, vec![native]);
        let error = client
            .enroll_accessory(EnrollAccessoryRequest {
                address: 1 << 48,
                info: Some(
                    common::accessory::AccessoryInfo::with_defaults(
                        "markers",
                        common::accessory::AccessoryType::MarkerControlBoard,
                    )
                    .into(),
                ),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::InvalidArgument);
        drop(stream);
        drop(client);
        cancel.cancel();
        task.await.unwrap();
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
    async fn event_rpc_demand_aggregates_screens_and_releases_idle_disconnected_clients() {
        use odyssey_hub_server_interface::SubscribeEventsRequest;
        use odyssey_hub_server_interface::service_client::ServiceClient;
        use odyssey_hub_server_interface::service_server::ServiceServer;
        use tokio_stream::wrappers::TcpListenerStream;
        use tokio_util::sync::CancellationToken;

        use crate::accessories::MarkerDemandCounts;

        async fn expect_counts(
            demand: &mut watch::Receiver<MarkerDemandCounts>,
            expected: MarkerDemandCounts,
        ) {
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                demand.wait_for(|counts| *counts == expected),
            )
            .await
            .expect("RPC demand transition timed out")
            .unwrap();
        }

        let server = server();
        let mut demand = server.marker_demand.subscribe();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let cancel = CancellationToken::new();
        let shutdown = cancel.clone();
        let task = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(ServiceServer::new(server))
                .serve_with_incoming_shutdown(
                    TcpListenerStream::new(listener),
                    shutdown.cancelled(),
                )
                .await
                .unwrap();
        });
        let mut client = ServiceClient::connect(format!("http://{address}"))
            .await
            .unwrap();
        let passive = client
            .subscribe_events(SubscribeEventsRequest {
                passive_tracking: true,
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        expect_counts(&mut demand, [0; 6]).await;
        let all = client
            .subscribe_events(SubscribeEventsRequest::default())
            .await
            .unwrap()
            .into_inner();
        expect_counts(&mut demand, [1; 6]).await;
        let last_screen = odyssey_common::MAX_SCREEN_ID;
        let scoped = client
            .subscribe_events(SubscribeEventsRequest {
                screen_id: Some(u32::from(last_screen)),
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        let mut expected = [1; 6];
        expected[usize::from(last_screen)] = 2;
        expect_counts(&mut demand, expected).await;
        let invalid = client
            .subscribe_events(SubscribeEventsRequest {
                screen_id: Some(u32::from(last_screen) + 1),
                ..Default::default()
            })
            .await
            .err()
            .unwrap();
        assert_eq!(invalid.code(), tonic::Code::InvalidArgument);
        expect_counts(&mut demand, expected).await;
        drop(all);
        let mut expected = [0; 6];
        expected[usize::from(last_screen)] = 1;
        expect_counts(&mut demand, expected).await;
        drop(scoped);
        expect_counts(&mut demand, [0; 6]).await;
        drop(passive);
        drop(client);
        cancel.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn cancelling_event_consumer_releases_demand_without_device_events() {
        use odyssey_hub_server_interface::SubscribeEventsRequest;
        use odyssey_hub_server_interface::service_server::Service as _;
        use tonic::Request;

        let server = server();
        let mut demand = server.marker_demand.subscribe();
        let stream = server
            .subscribe_events(Request::new(SubscribeEventsRequest::default()))
            .await
            .unwrap()
            .into_inner();
        let task = tokio::spawn(async move {
            let _stream = stream;
            std::future::pending::<()>().await;
        });
        assert_eq!(*demand.borrow_and_update(), [1; 6]);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(*demand.borrow(), [0; 6]);
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
