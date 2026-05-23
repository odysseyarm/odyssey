use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter, ValueNotification};
use btleplug::platform::{Manager, Peripheral};
use futures::stream::StreamExt;
use futures_concurrency::prelude::*;
use odyssey_hub_common::accessory::{AccessoryInfo, AccessoryInfoMap, AccessoryMap, AccessoryType};
use std::sync::Arc;
use std::{collections::HashMap, time::Duration};
use tokio::{
    sync::{broadcast::Sender, watch::Receiver},
    time,
};
use tokio_stream::wrappers::{IntervalStream, WatchStream};
use tracing::{debug, info, warn};
use uuid::Uuid;

const NUS_TX_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);

const MANTIS_SHOT_CHAR_UUID: Uuid = Uuid::from_u128(0x6e40000e_204d_616e_7469_732054656368);

/// Events from the accessory_manager merged stream
enum AccessoryEvent {
    InfoChanged,
    ScanComplete,
}

pub async fn accessory_manager(
    event_tx: tokio::sync::broadcast::Sender<odyssey_hub_common::events::Event>,
    info_watch: Receiver<AccessoryInfoMap>,
    map_tx: Sender<AccessoryMap>,
    dl: Arc<
        parking_lot::Mutex<
            Vec<(
                odyssey_hub_common::device::Device,
                tokio::sync::mpsc::Sender<crate::device_tasks::DeviceTaskMessage>,
            )>,
        >,
    >,
) {
    let manager = match Manager::new().await {
        Ok(m) => m,
        Err(e) => {
            warn!("accessory_manager: failed to init BT manager: {e}");
            return;
        }
    };
    let adapter = loop {
        match manager.adapters().await {
            Ok(mut adapters) => {
                if let Some(a) = adapters.into_iter().next() {
                    break a;
                }
            }
            Err(e) => {
                warn!("accessory_manager: failed to list BT adapters: {e}");
                return;
            }
        }
        time::sleep(Duration::from_secs(1)).await;
    };

    let filter = ScanFilter { services: vec![] };

    let mut info_map: AccessoryInfoMap = info_watch.borrow().clone();
    let mut last_statuses: HashMap<[u8; 6], bool> =
        info_map.keys().map(|&id| (id, false)).collect();

    let mut periph_cache: HashMap<[u8; 6], Peripheral> = HashMap::new();

    send_if_changed(&info_map, &last_statuses, &map_tx);

    // Convert info_watch to stream - clone it first so we can use it later
    let info_watch_clone = info_watch.clone();
    let info_stream = WatchStream::new(info_watch_clone).map(|_| AccessoryEvent::InfoChanged);

    // Create periodic scan interval stream (2 second intervals)
    let scan_interval = tokio::time::interval(Duration::from_secs(2));
    let scan_stream = IntervalStream::new(scan_interval).then({
        let adapter = adapter.clone();
        let filter = filter.clone();
        move |_| {
            let adapter = adapter.clone();
            let filter = filter.clone();
            async move {
                let _ = adapter.start_scan(filter).await;
                time::sleep(Duration::from_secs(3)).await;
                let _ = adapter.stop_scan().await;
                AccessoryEvent::ScanComplete
            }
        }
    });

    // Merge both streams
    let merged = (info_stream, scan_stream).merge();
    tokio::pin!(merged);

    while let Some(event) = merged.next().await {
        match event {
            AccessoryEvent::InfoChanged => {
                info_map = info_watch.borrow().clone();
                last_statuses.retain(|id, _| info_map.contains_key(id));
                for &id in info_map.keys() {
                    last_statuses.entry(id).or_insert(false);
                }
                periph_cache.retain(|id, _| info_map.contains_key(id));
                send_if_changed(&info_map, &last_statuses, &map_tx);
            }
            AccessoryEvent::ScanComplete => {
                let mut by_mac: HashMap<[u8; 6], Peripheral> = HashMap::new();
                if let Ok(peris) = adapter.peripherals().await {
                    for p in peris {
                        if let Ok(Some(props)) = p.properties().await {
                            by_mac.insert(props.address.into_inner(), p.clone());
                        }
                    }
                }

                // Snapshot: (mac, assignment, type)
                let snapshot: Vec<([u8; 6], AccessoryType)> =
                    info_map
                        .iter()
                        .map(|(id, info)| (*id, info.ty))
                        .collect();

                for (id, ty) in snapshot {
                    if let Some(p) = by_mac.get(&id).cloned() {
                        periph_cache.entry(id).or_insert_with(|| p.clone());

                        let mut connected = p.is_connected().await.unwrap_or(false);
                        if !connected {
                            if let Err(e) = p.connect().await {
                                warn!("Failed to connect to {id:?}: {e}");
                            } else {
                                if let Err(e) = p.discover_services().await {
                                    warn!("Failed to discover services for {id:?}: {e}");
                                } else {
                                    // pick characteristic by accessory type
                                    let target_char_uuid = match ty {
                                        AccessoryType::DryFireMag => NUS_TX_UUID,
                                        AccessoryType::MantisX => MANTIS_SHOT_CHAR_UUID,
                                    };

                                    if let Some(ch) = p
                                        .characteristics()
                                        .into_iter()
                                        .find(|c| c.uuid == target_char_uuid)
                                    {
                                        if let Err(e) = p.subscribe(&ch).await {
                                            warn!("Subscribe failed for {id:?}: {e}");
                                        } else {
                                            info!("Subscribed to {:?} notifications on {id:?}", ty);
                                            let notif_stream = match p.notifications().await {
                                                Ok(s) => s,
                                                Err(e) => {
                                                    warn!("Failed to get notifications stream for {id:?}: {e}");
                                                    continue;
                                                }
                                            };
                                            let dl = dl.clone();
                                            let event_tx = event_tx.clone();
                                            let info_watch = info_watch.clone();
                                            tokio::spawn(async move {
                                                let mut notif_stream = notif_stream;
                                                while let Some(ValueNotification { uuid, value }) =
                                                    notif_stream.next().await
                                                {
                                                    if uuid == target_char_uuid {
                                                        let mut shot = false;
                                                        match ty {
                                                            AccessoryType::DryFireMag => {
                                                                shot = true;
                                                            }
                                                            AccessoryType::MantisX => {
                                                                if value.len() == 1
                                                                    && value[0] == 0x01
                                                                {
                                                                    shot = true;
                                                                }
                                                            }
                                                        }

                                                        if shot {
                                                            debug!("shotted");
                                                            let assignment_opt = info_watch
                                                                .borrow()
                                                                .get(&id)
                                                                .and_then(|i| i.assignment);
                                                            if let Some(assignment) = assignment_opt {
                                                                let maybe_device = {
                                                                    let dl_guard = dl.lock();
                                                                    let assignment_uuid: [u8; 6] =
                                                                        assignment
                                                                            .get()
                                                                            .to_le_bytes()[..6]
                                                                            .try_into()
                                                                            .unwrap();
                                                                    dl_guard
                                                                        .iter()
                                                                        .find(|d| {
                                                                            d.0.uuid
                                                                                == assignment_uuid
                                                                        })
                                                                        .map(|d| d.0.clone())
                                                                };
                                                                if let Some(device) = maybe_device {
                                                                    tokio::spawn({
                                                                        let event_tx =
                                                                            event_tx.clone();
                                                                        async move {
                                                                            if let Err(e) = event_tx.send(
                                                                                    odyssey_hub_common::events::Event::DeviceEvent(
                                                                                        odyssey_hub_common::events::DeviceEvent(
                                                                                            device,
                                                                                            odyssey_hub_common::events::DeviceEventKind::ImpactEvent(
                                                                                                odyssey_hub_common::events::ImpactEvent { timestamp: u32::MAX },
                                                                                            ),
                                                                                        )
                                                                                    )
                                                                            ) {
                                                                                warn!("accessory_manager: failed to forward impact event: {e}");
                                                                            }
                                                                        }
                                                                    });
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                warn!("{:?} notifications ended for {:?}", ty, id);
                                            });
                                        }
                                    } else {
                                        warn!(
                                            "No target characteristic ({:?}) found for {id:?}",
                                            ty
                                        );
                                    }
                                }
                            }
                            connected = p.is_connected().await.unwrap_or(false);
                        }
                        *last_statuses.entry(id).or_insert(false) = connected;
                    } else {
                        *last_statuses.entry(id).or_insert(false) = false;
                    }
                }

                send_if_changed(&info_map, &last_statuses, &map_tx);
            }
        }
    }
}

fn send_if_changed(
    info_map: &HashMap<[u8; 6], AccessoryInfo>,
    statuses: &HashMap<[u8; 6], bool>,
    tx: &Sender<AccessoryMap>,
) {
    let current: AccessoryMap = info_map
        .iter()
        .map(|(&id, info)| {
            let connected = *statuses.get(&id).unwrap_or(&false);
            (id, (info.clone(), connected))
        })
        .collect();

    let _ = tx.send(current);
}
