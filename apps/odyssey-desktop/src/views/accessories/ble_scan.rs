//! Looking for accessories over Bluetooth, from the UI process.

use std::collections::HashMap;
use std::time::Duration;

use btleplug::api::{Central as _, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager};
use dioxus::prelude::*;
use odyssey_hub_common::accessory::AccessoryType;
use uuid::Uuid;

/// Accessories found nearby: id -> (label, type).
pub(super) type Nearby = HashMap<[u8; 6], (String, AccessoryType)>;

/// The name a DryFireMag advertises.
const DRY_FIRE_MAG_NAME: &str = "DryFireMag";
/// The service a MantisX advertises.
const MANTIS_SERVICE_UUID: Uuid = Uuid::from_bytes([
    0x6e, 0x40, 0x00, 0x01, 0x20, 0x4d, 0x61, 0x6e, 0x74, 0x69, 0x73, 0x20, 0x54, 0x65, 0x63, 0x68,
]);

/// How long each scan listens before the results are read.
const SCAN_DURATION: Duration = Duration::from_secs(3);
/// How long to rest between scans.
const SCAN_PAUSE: Duration = Duration::from_secs(5);

/// Keeps `nearby` up to date with the DryFireMag and MantisX accessories in
/// range. Does nothing when the machine has no Bluetooth adapter.
pub(super) fn use_ble_scan(mut nearby: Signal<Nearby>) {
    use_future(move || async move {
        let Ok(manager) = Manager::new().await else {
            return;
        };
        let Ok(adapters) = manager.adapters().await else {
            return;
        };
        let Some(adapter) = adapters.into_iter().next() else {
            return;
        };
        loop {
            nearby.set(scan_once(&adapter).await);
            tokio::time::sleep(SCAN_PAUSE).await;
        }
    });
}

async fn scan_once(adapter: &Adapter) -> Nearby {
    let filter = ScanFilter { services: vec![] };
    // A scan that cannot start or stop leaves the previous results readable.
    let _ = adapter.start_scan(filter).await;
    tokio::time::sleep(SCAN_DURATION).await;
    let _ = adapter.stop_scan().await;

    let mut found = Nearby::new();
    let Ok(peripherals) = adapter.peripherals().await else {
        return found;
    };
    for peripheral in peripherals {
        let Ok(Some(properties)) = peripheral.properties().await else {
            continue;
        };
        let id = properties.address.into_inner();
        if properties.local_name.as_deref() == Some(DRY_FIRE_MAG_NAME) {
            found.insert(
                id,
                (DRY_FIRE_MAG_NAME.to_string(), AccessoryType::DryFireMag),
            );
        } else if properties.services.contains(&MANTIS_SERVICE_UUID) {
            found.insert(id, ("MantisX".to_string(), AccessoryType::MantisX));
        }
    }
    found
}
