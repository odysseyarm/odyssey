use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use backon::{ConstantBuilder, Retryable as _};
use btleplug::api::{Central as _, Manager as _, Peripheral as _, ScanFilter, ValueNotification};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::stream::StreamExt as _;
use futures_concurrency::stream::Merge as _;
use odyssey_hub_common::accessory::{AccessoryInfoMap, AccessoryMap, AccessoryType};
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event, ImpactEvent};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time;
use tokio_stream::wrappers::{IntervalStream, WatchStream};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::device_list::DeviceList;

const NUS_TX_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);

const MANTIS_SHOT_CHAR_UUID: Uuid = Uuid::from_u128(0x6e40000e_204d_616e_7469_732054656368);

/// How long to wait for a Bluetooth adapter to show up.
const ADAPTER_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Time between the starts of two scans.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// How long each scan listens.
const SCAN_DURATION: Duration = Duration::from_secs(3);

/// The impact timestamp of a shot reported by an accessory; accessories have no device clock.
const ACCESSORY_IMPACT_TIMESTAMP: u32 = u32::MAX;

/// MantisX reports a shot as a single notification byte with this value.
const MANTIS_SHOT_MARKER: u8 = 0x01;

/// Events from the accessory manager's merged stream.
enum AccessoryEvent {
    InfoChanged,
    ScanComplete,
}

/// Watches for the user's accessories over Bluetooth, keeps them connected, and turns a
/// shot notification into an impact event for the device the accessory is assigned to.
///
/// Returns when Bluetooth is unavailable or `cancel` fires. Notification tasks and
/// Bluetooth connections are owned by this manager and stopped during teardown.
pub(crate) async fn accessory_manager(
    event_tx: broadcast::Sender<Event>,
    info_watch: watch::Receiver<AccessoryInfoMap>,
    map_tx: broadcast::Sender<AccessoryMap>,
    device_list: DeviceList,
    shared_map: Arc<Mutex<AccessoryMap>>,
    cancel: CancellationToken,
) {
    let adapter = tokio::select! {
        () = cancel.cancelled() => return,
        adapter = find_adapter() => match adapter {
            Some(adapter) => adapter,
            None => return,
        },
    };

    let info_map: AccessoryInfoMap = info_watch.borrow().clone();
    let mut accessories = AccessoryManager {
        adapter,
        device_list,
        event_tx,
        map_tx,
        last_statuses: info_map.keys().map(|&id| (id, false)).collect(),
        connections: HashMap::new(),
        shared_map,
        info_map,
        info_watch,
    };
    accessories.send_if_changed();
    tokio::select! {
        () = cancel.cancelled() => {}
        () = accessories.run() => {}
    }
    accessories.disconnect_all().await;
    if let Err(error) = accessories.adapter.stop_scan().await {
        warn!("Failed to stop accessory scan: {error}");
    }
}

async fn find_adapter() -> Option<Adapter> {
    let manager = match Manager::new().await {
        Ok(manager) => manager,
        Err(error) => {
            warn!("accessory_manager: failed to init BT manager: {error}");
            return None;
        }
    };
    let found = (async || match manager.adapters().await {
        Ok(adapters) => adapters.into_iter().next().ok_or(AdapterMiss::NoneYet),
        Err(error) => Err(AdapterMiss::ListFailed(error)),
    })
    .retry(
        ConstantBuilder::default()
            .with_delay(ADAPTER_POLL_INTERVAL)
            .without_max_times(),
    )
    .when(|miss| matches!(miss, AdapterMiss::NoneYet))
    .await;
    match found {
        Ok(adapter) => Some(adapter),
        Err(AdapterMiss::ListFailed(error)) => {
            warn!("accessory_manager: failed to list BT adapters: {error}");
            None
        }
        // Unreachable: `NoneYet` is retried without limit.
        Err(AdapterMiss::NoneYet) => None,
    }
}

/// Why one adapter lookup produced no adapter.
enum AdapterMiss {
    /// The system lists no Bluetooth adapter yet.
    NoneYet,
    /// The adapter list could not be read.
    ListFailed(btleplug::Error),
}

struct AccessoryManager {
    adapter: Adapter,
    device_list: DeviceList,
    event_tx: broadcast::Sender<Event>,
    map_tx: broadcast::Sender<AccessoryMap>,
    info_watch: watch::Receiver<AccessoryInfoMap>,
    /// The accessories the user configured, as of the last change.
    info_map: AccessoryInfoMap,
    /// Whether each configured accessory was connected after the last scan.
    last_statuses: HashMap<[u8; 6], bool>,
    shared_map: Arc<Mutex<AccessoryMap>>,
    connections: HashMap<[u8; 6], AccessoryConnection>,
}

/// Dropping the manager (including cancellation of its future) must not leave
/// notification readers holding the old device list and event channel alive.
struct AccessoryConnection {
    peripheral: Peripheral,
    notifications: Option<JoinHandle<()>>,
    ty: AccessoryType,
}

impl Drop for AccessoryConnection {
    fn drop(&mut self) {
        if let Some(notifications) = &self.notifications {
            notifications.abort();
        }
    }
}

impl AccessoryManager {
    async fn run(&mut self) {
        let info_stream =
            WatchStream::new(self.info_watch.clone()).map(|_| AccessoryEvent::InfoChanged);

        let adapter = self.adapter.clone();
        let scan_stream = IntervalStream::new(time::interval(SCAN_INTERVAL)).then(move |_| {
            let adapter = adapter.clone();
            async move {
                let _ = adapter.start_scan(ScanFilter { services: vec![] }).await;
                time::sleep(SCAN_DURATION).await;
                let _ = adapter.stop_scan().await;
                AccessoryEvent::ScanComplete
            }
        });

        let merged = (info_stream, scan_stream).merge();
        tokio::pin!(merged);
        while let Some(event) = merged.next().await {
            match event {
                AccessoryEvent::InfoChanged => self.on_info_changed().await,
                AccessoryEvent::ScanComplete => self.on_scan_complete().await,
            }
        }
    }

    async fn on_info_changed(&mut self) {
        self.info_map = self.info_watch.borrow().clone();
        self.last_statuses
            .retain(|id, _| self.info_map.contains_key(id));
        for &id in self.info_map.keys() {
            self.last_statuses.entry(id).or_insert(false);
        }
        let removed: Vec<_> = self
            .connections
            .iter()
            .filter(|(id, connection)| {
                self.info_map
                    .get(*id)
                    .is_none_or(|info| info.ty != connection.ty)
            })
            .map(|(&id, _)| id)
            .collect();
        for id in removed {
            self.disconnect(id).await;
            if let Some(connected) = self.last_statuses.get_mut(&id) {
                *connected = false;
            }
        }
        self.send_if_changed();
    }

    async fn on_scan_complete(&mut self) {
        let by_address = self.peripherals_by_address().await;
        let configured: Vec<([u8; 6], AccessoryType)> = self
            .info_map
            .iter()
            .map(|(id, info)| (*id, info.ty))
            .collect();

        for (id, ty) in configured {
            // Connected accessories need not keep advertising. Keep using the
            // owned peripheral even when discovery does not return it this scan.
            let peripheral = by_address.get(&id).cloned().or_else(|| {
                self.connections
                    .get(&id)
                    .map(|connection| connection.peripheral.clone())
            });
            let Some(peripheral) = peripheral else {
                self.last_statuses.insert(id, false);
                continue;
            };
            let connected = peripheral.is_connected().await.unwrap_or(false);
            let subscribed = self
                .connections
                .get(&id)
                .and_then(|connection| connection.notifications.as_ref())
                .is_some_and(|notifications| !notifications.is_finished());
            if !connected || !subscribed {
                self.disconnect(id).await;
                let ready = self.connect_and_subscribe(&peripheral, id, ty).await;
                self.last_statuses.insert(id, ready);
            } else {
                self.last_statuses.insert(id, true);
            }
        }

        self.send_if_changed();
    }

    async fn peripherals_by_address(&self) -> HashMap<[u8; 6], Peripheral> {
        let mut by_address = HashMap::new();
        let Ok(peripherals) = self.adapter.peripherals().await else {
            return by_address;
        };
        for peripheral in peripherals {
            if let Ok(Some(properties)) = peripheral.properties().await {
                by_address.insert(properties.address.into_inner(), peripheral);
            }
        }
        by_address
    }

    /// A connection is ready only after its notification reader is running.
    /// Failed setup disconnects so a later scan can retry the whole subscription.
    async fn connect_and_subscribe(
        &mut self,
        peripheral: &Peripheral,
        id: [u8; 6],
        ty: AccessoryType,
    ) -> bool {
        // Register before the first await so cancellation during setup also
        // disconnects a peripheral that has not acquired a notification reader.
        self.connections.insert(
            id,
            AccessoryConnection {
                peripheral: peripheral.clone(),
                notifications: None,
                ty,
            },
        );
        if !peripheral.is_connected().await.unwrap_or(false)
            && let Err(e) = peripheral.connect().await
        {
            warn!("Failed to connect to {id:?}: {e}");
            self.disconnect(id).await;
            return false;
        }
        if let Err(e) = peripheral.discover_services().await {
            warn!("Failed to discover services for {id:?}: {e}");
            self.disconnect(id).await;
            return false;
        }

        let shot_characteristic = match ty {
            AccessoryType::DryFireMag => NUS_TX_UUID,
            AccessoryType::MantisX => MANTIS_SHOT_CHAR_UUID,
        };
        let Some(characteristic) = peripheral
            .characteristics()
            .into_iter()
            .find(|c| c.uuid == shot_characteristic)
        else {
            warn!("No target characteristic ({ty:?}) found for {id:?}");
            self.disconnect(id).await;
            return false;
        };
        if let Err(e) = peripheral.subscribe(&characteristic).await {
            warn!("Subscribe failed for {id:?}: {e}");
            self.disconnect(id).await;
            return false;
        }
        info!("Subscribed to {ty:?} notifications on {id:?}");
        let notifications = match peripheral.notifications().await {
            Ok(notifications) => notifications,
            Err(e) => {
                warn!("Failed to get notifications stream for {id:?}: {e}");
                self.disconnect(id).await;
                return false;
            }
        };

        // One owned reader per accessory; reconnects and removal stop the old reader.
        let forwarder = ShotForwarder {
            device_list: self.device_list.clone(),
            event_tx: self.event_tx.clone(),
            info_watch: self.info_watch.clone(),
            id,
            ty,
            shot_characteristic,
        };
        self.connections.insert(
            id,
            AccessoryConnection {
                peripheral: peripheral.clone(),
                notifications: Some(tokio::spawn(forwarder.run(notifications))),
                ty,
            },
        );
        true
    }

    async fn disconnect(&mut self, id: [u8; 6]) {
        if let Some(connection) = self.connections.get(&id) {
            if let Some(notifications) = &connection.notifications {
                notifications.abort();
            }
            if let Err(error) = connection.peripheral.disconnect().await {
                warn!("Failed to disconnect accessory {id:?}: {error}");
            }
        }
        self.connections.remove(&id);
    }

    async fn disconnect_all(&mut self) {
        for connection in self.connections.values() {
            if let Some(notifications) = &connection.notifications {
                notifications.abort();
            }
        }
        let ids: Vec<_> = self.connections.keys().copied().collect();
        for id in ids {
            self.disconnect(id).await;
        }
        for connected in self.last_statuses.values_mut() {
            *connected = false;
        }
        self.send_if_changed();
    }

    fn send_if_changed(&self) {
        let current: AccessoryMap = self
            .info_watch
            .borrow()
            .iter()
            .map(|(&id, info)| {
                let connected = *self.last_statuses.get(&id).unwrap_or(&false);
                (id, (info.clone(), connected))
            })
            .collect();
        *self
            .shared_map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = current.clone();
        let _ = self.map_tx.send(current);
    }
}

/// Turns one accessory's notifications into impact events on the device it is assigned to.
struct ShotForwarder {
    device_list: DeviceList,
    event_tx: broadcast::Sender<Event>,
    info_watch: watch::Receiver<AccessoryInfoMap>,
    id: [u8; 6],
    ty: AccessoryType,
    shot_characteristic: Uuid,
}

impl ShotForwarder {
    async fn run(self, mut notifications: impl futures::Stream<Item = ValueNotification> + Unpin) {
        while let Some(ValueNotification { uuid, value }) = notifications.next().await {
            if uuid != self.shot_characteristic || !self.is_shot(&value) {
                continue;
            }
            debug!("shotted");
            self.forward_impact();
        }
        warn!("{:?} notifications ended for {:?}", self.ty, self.id);
    }

    fn is_shot(&self, value: &[u8]) -> bool {
        match self.ty {
            AccessoryType::DryFireMag => true,
            AccessoryType::MantisX => value == [MANTIS_SHOT_MARKER],
        }
    }

    /// The device the accessory is assigned to hears the shot as an impact.
    fn forward_impact(&self) {
        let assignment = self
            .info_watch
            .borrow()
            .get(&self.id)
            .filter(|info| info.ty == self.ty)
            .and_then(|info| info.assignment);
        let Some(assignment) = assignment else {
            return;
        };
        let assignment_bytes = assignment.get().to_le_bytes();
        // The assignment is a device uuid in the low 6 bytes.
        let Some(&device_uuid) = assignment_bytes.first_chunk::<6>() else {
            return;
        };
        let Some(device) = self.device_list.find(device_uuid) else {
            return;
        };
        let impact = DeviceEventKind::ImpactEvent(ImpactEvent {
            timestamp: ACCESSORY_IMPACT_TIMESTAMP,
        });
        if let Err(e) = self
            .event_tx
            .send(Event::DeviceEvent(DeviceEvent(device, impact)))
        {
            warn!("accessory_manager: failed to forward impact event: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};

    use super::*;

    fn forwarder(ty: AccessoryType) -> ShotForwarder {
        let (event_tx, _) = broadcast::channel(4);
        let (_, info_watch) = watch::channel(AccessoryInfoMap::new());
        ShotForwarder {
            device_list: DeviceList::default(),
            event_tx,
            info_watch,
            id: [0; 6],
            ty,
            shot_characteristic: NUS_TX_UUID,
        }
    }

    #[test]
    fn dry_fire_mag_notifies_on_every_shot_and_mantis_only_on_its_marker() {
        let dry_fire = forwarder(AccessoryType::DryFireMag);
        assert!(dry_fire.is_shot(&[]));
        assert!(dry_fire.is_shot(&[7, 7]));

        let mantis = forwarder(AccessoryType::MantisX);
        assert!(mantis.is_shot(&[MANTIS_SHOT_MARKER]));
        assert!(!mantis.is_shot(&[0]));
        assert!(!mantis.is_shot(&[MANTIS_SHOT_MARKER, 0]));
    }

    #[test]
    fn a_shot_becomes_an_impact_on_the_assigned_device() {
        let device = Device {
            uuid: [1, 2, 3, 4, 5, 6],
            transport: Transport::Usb,
            capabilities: DeviceCapabilities::empty(),
            firmware_version: [0; 3],
            events_transport: EventsTransport::Wired,
            events_connected: false,
            product_id: 0,
            name: Device::name_bytes(""),
            netcore_release: 0,
        };
        let device_list = DeviceList::default();
        device_list.upsert(device, None);

        let mut info = odyssey_hub_common::accessory::AccessoryInfo::with_defaults(
            "mag",
            AccessoryType::DryFireMag,
        );
        info.assignment = std::num::NonZeroU64::new(u64::from_le_bytes([1, 2, 3, 4, 5, 6, 0, 0]));
        let (_info_sender, info_watch) = watch::channel(HashMap::from([([0; 6], info)]));
        let (event_tx, mut events) = broadcast::channel(4);
        let forwarder = ShotForwarder {
            device_list,
            event_tx,
            info_watch,
            id: [0; 6],
            ty: AccessoryType::DryFireMag,
            shot_characteristic: NUS_TX_UUID,
        };

        forwarder.forward_impact();

        let Ok(Event::DeviceEvent(DeviceEvent(device, DeviceEventKind::ImpactEvent(impact)))) =
            events.try_recv()
        else {
            panic!("expected an impact event");
        };
        assert_eq!(device.uuid, [1, 2, 3, 4, 5, 6]);
        assert_eq!(impact.timestamp, ACCESSORY_IMPACT_TIMESTAMP);
    }

    #[test]
    fn an_unassigned_accessory_forwards_nothing() {
        let forwarder = forwarder(AccessoryType::DryFireMag);
        let mut events = forwarder.event_tx.subscribe();
        forwarder.forward_impact();
        assert!(events.try_recv().is_err());
    }
}
