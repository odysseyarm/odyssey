use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter, Characteristic, ValueNotification};
use btleplug::platform::{Manager, Peripheral};
use odyssey_hub_common::accessory::{AccessoryInfo, AccessoryInfoMap, AccessoryMap};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
use tokio::{
    sync::{broadcast::Sender, watch::Receiver},
    time,
};
use uuid::Uuid;
use futures::StreamExt;
use tracing::{info, warn, error};

const NUS_TX_UUID: Uuid = Uuid::from_u128(0x6e400003_b5a3_f393_e0a9_e50e24dcca9e);

pub async fn accessory_manager(
    mut info_watch: Receiver<AccessoryInfoMap>,
    event_tx: Sender<AccessoryMap>,
) {
    let _nus_uuid = Uuid::from_bytes([
        0x6e, 0x40, 0x00, 0x01, 0xb5, 0xa3, 0xf3, 0x93,
        0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc, 0xca, 0x9e,
    ]);

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

    // Cache of connected peripherals
    let mut periph_cache: HashMap<[u8; 6], Peripheral> = HashMap::new();

    send_if_changed(&info_map, &last_statuses, &event_tx);

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
                send_if_changed(&info_map, &last_statuses, &event_tx);
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

                for (&id, _info) in info_map.iter() {
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
                                    // Subscribe if NUS TX is found
                                    if let Some(tx_char) = p.characteristics().into_iter().find(|c| c.uuid == NUS_TX_UUID) {
                                        if let Err(e) = p.subscribe(&tx_char).await {
                                            warn!("Subscribe failed for {id:?}: {e}");
                                        } else {
                                            info!("Subscribed to DryFireMag NUS TX on {id:?}");

                                            let notif_stream = p.notifications().await.unwrap();
                                            tokio::spawn(async move {
                                                let mut notif_stream = notif_stream;
                                                while let Some(ValueNotification { uuid, value }) = notif_stream.next().await {
                                                    if uuid == NUS_TX_UUID {
                                                        info!("DryFireMag {:?} trigger packet: {:?}", id, value);
                                                    }
                                                }
                                                warn!("DryFireMag {:?} notifications ended", id);
                                            });
                                        }
                                    } else {
                                        warn!("No NUS TX characteristic found for {id:?}");
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

                send_if_changed(&info_map, &last_statuses, &event_tx);
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
