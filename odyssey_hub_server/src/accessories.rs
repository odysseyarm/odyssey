use btleplug::api::{Manager as _, Central, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use odyssey_hub_common::{AccessoryInfo, AccessoryInfoMap, AccessoryMap};
use tokio::{sync::{broadcast::Sender, watch::Receiver}, time};
use uuid::Uuid;
use std::{collections::{HashMap, HashSet}, time::Duration};

pub async fn accessory_manager(
    mut info_watch: Receiver<AccessoryInfoMap>,
    event_tx: Sender<AccessoryMap>,
) {
    // DryFireMag uses Nordic UART Service
    let nus_uuid = Uuid::from_bytes([
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
    let filter = ScanFilter { services: vec![nus_uuid] };

    let mut info_map = info_watch.borrow().clone();
    let mut last_statuses: HashMap<[u8; 6], bool> =
        info_map.keys().map(|&id| (id, false)).collect();

    // Initial broadcast: all known accessories disconnected
    send_if_changed(&info_map, &HashSet::new(), &mut last_statuses, &event_tx);

    loop {
        tokio::select! {
            changed = info_watch.changed() => {
                if changed.is_ok() {
                    info_map = info_watch.borrow().clone();
                    // Ensure new entries start as disconnected
                    for &id in info_map.keys() {
                        last_statuses.entry(id).or_insert(false);
                    }
                    send_if_changed(&info_map, &HashSet::new(), &mut last_statuses, &event_tx);
                } else {
                    break;
                }
            }
            _ = async {
                adapter.start_scan(filter.clone()).await.unwrap();
                time::sleep(Duration::from_secs(3)).await;
                adapter.stop_scan().await.unwrap();
            } => {
                let mut seen = HashSet::new();
                for p in adapter.peripherals().await.unwrap() {
                    if let Some(props) = p.properties().await.unwrap() {
                        if props.local_name.as_deref() == Some("DryFireMag") {
                            seen.insert(props.address.into_inner());
                        }
                    }
                }
                send_if_changed(&info_map, &seen, &mut last_statuses, &event_tx);
            }
        }

        time::sleep(Duration::from_secs(5)).await;
    }
}

fn send_if_changed(
    info_map: &HashMap<[u8; 6], AccessoryInfo>,
    seen: &HashSet<[u8; 6]>,
    last_statuses: &mut HashMap<[u8; 6], bool>,
    tx: &Sender<AccessoryMap>,
) {
    let mut updated = HashMap::new();
    for (&id, _) in info_map.iter() {
        updated.insert(id, seen.contains(&id));
    }
    if *last_statuses == updated {
        return;
    }
    *last_statuses = updated.clone();

    let map: AccessoryMap = updated.into_iter()
        .map(|(id, connected)| {
            let info = info_map.get(&id).unwrap().clone();
            (id, (info, connected))
        })
        .collect();
    let _ = tx.send(map);
}
