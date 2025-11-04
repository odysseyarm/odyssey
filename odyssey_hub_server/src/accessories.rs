use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter, ValueNotification};
use btleplug::platform::{Manager, Peripheral};
use futures::StreamExt;
use odyssey_hub_common::accessory::{AccessoryInfo, AccessoryInfoMap, AccessoryMap, AccessoryType};
use std::sync::Arc;
use std::{collections::HashMap, time::Duration};
use tokio::{
    sync::{broadcast::Sender, watch::Receiver},
    time,
};
use tracing::{debug, info, warn};
use uuid::Uuid;

const NUS_TX_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);

const BBX_SHOT_CHAR_UUID: Uuid = Uuid::from_u128(0x6e40000e_204d_616e_7469_732054656368);

pub async fn accessory_manager(
    event_tx: tokio::sync::broadcast::Sender<odyssey_hub_common::events::Event>,
    mut info_watch: Receiver<AccessoryInfoMap>,
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
    let manager = Manager::new().await.unwrap();
    let adapter = loop {
        if let Some(a) = manager.adapters().await.unwrap().into_iter().next() {
            break a;
        }
        time::sleep(Duration::from_secs(1)).await;
    };

    let filter = ScanFilter { services: vec![] };

    let mut info_map: AccessoryInfoMap = info_watch.borrow().clone();
    let mut last_statuses: HashMap<[u8; 6], bool> =
        info_map.keys().map(|&id| (id, false)).collect();

    let mut periph_cache: HashMap<[u8; 6], Peripheral> = HashMap::new();

    send_if_changed(&info_map, &last_statuses, &map_tx);

    loop {
        tokio::select! {
            changed = info_watch.changed() => {
                if changed.is_err() {
                    break;
                }
                info_map = info_watch.borrow().clone();
                last_statuses.retain(|id, _| info_map.contains_key(id));
                for &id in info_map.keys() {
                    last_statuses.entry(id).or_insert(false);
                }
                periph_cache.retain(|id, _| info_map.contains_key(id));
                send_if_changed(&info_map, &last_statuses, &map_tx);
            }

            _ = async {
                let _ = adapter.start_scan(filter.clone()).await;
                time::sleep(Duration::from_secs(3)).await;
                let _ = adapter.stop_scan().await;
            } => {
                let mut by_mac: HashMap<[u8;6], Peripheral> = HashMap::new();
                if let Ok(peris) = adapter.peripherals().await {
                    for p in peris {
                        if let Ok(Some(props)) = p.properties().await {
                            by_mac.insert(props.address.into_inner(), p.clone());
                        }
                    }
                }

                // Snapshot: (mac, assignment, type)
                let snapshot: Vec<([u8; 6], Option<std::num::NonZeroU64>, AccessoryType)> =
                    info_map.iter().map(|(id, info)| (*id, info.assignment, info.ty)).collect();

                for (id, assignment_opt, ty) in snapshot {
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
                                        AccessoryType::BlackbeardX => BBX_SHOT_CHAR_UUID,
                                    };

                                    if let Some(ch) = p.characteristics().into_iter().find(|c| c.uuid == target_char_uuid) {
                                        if let Err(e) = p.subscribe(&ch).await {
                                            warn!("Subscribe failed for {id:?}: {e}");
                                        } else {
                                            info!("Subscribed to {:?} notifications on {id:?}", ty);
                                            let notif_stream = p.notifications().await.unwrap();
                                            let dl = dl.clone();
                                            let event_tx = event_tx.clone();
                                            tokio::spawn(async move {
                                                let mut notif_stream = notif_stream;
                                                while let Some(ValueNotification { uuid, value }) = notif_stream.next().await {
                                                    if uuid == target_char_uuid {
                                                        let mut shot = false;
                                                        match ty {
                                                            AccessoryType::DryFireMag => {
                                                                // treat any packet as shot for now
                                                                shot = true;
                                                            }
                                                            AccessoryType::BlackbeardX => {
                                                                // shot when single byte 0x01
                                                                if value.len() == 1 && value[0] == 0x01 { shot = true; }
                                                            }
                                                        }

                                                        if shot {
                                                            debug!("shotted");
                                                            if let Some(assignment) = assignment_opt {
                                                                let maybe_device = {
                                                                    let dl_guard = dl.lock();
                                                                    dl_guard.iter()
                                                                        .find(|d| d.0.uuid == assignment.get())
                                                                        .map(|d| d.0.clone())
                                                                };
                                                                if let Some(device) = maybe_device {
                                                                    tokio::spawn({
                                                                        let event_tx = event_tx.clone();
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
                                        warn!("No target characteristic ({:?}) found for {id:?}", ty);
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

        time::sleep(Duration::from_secs(2)).await;
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
