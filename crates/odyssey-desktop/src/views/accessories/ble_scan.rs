//! Looking for accessories over Bluetooth, from the UI process.

use std::collections::HashMap;
use std::time::Duration;

use btleplug::api::{Central as _, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager};
use gpui::{Context, Task};
use odyssey_hub_common::accessory::AccessoryType;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::AccessoriesView;
use crate::runtime;

/// Accessories found nearby: id -> (label, type).
pub(super) type Nearby = HashMap<[u8; 6], (String, AccessoryType)>;

/// Where the search for nearby accessories stands.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum BleScan {
    /// No scan has finished yet.
    Starting,
    /// Scanning is impossible, for the reason given.
    Unavailable(String),
    /// What the latest scan found.
    Found(Nearby),
}

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

/// The scan future is aborted when the page goes away. Stopping discovery
/// must outlive that, otherwise navigation during the scan leaves it on.
struct ScanSession {
    adapter: Adapter,
    active: bool,
}

impl Drop for ScanSession {
    fn drop(&mut self) {
        if self.active {
            let adapter = self.adapter.clone();
            tokio::spawn(async move {
                if let Err(e) = adapter.stop_scan().await {
                    tracing::warn!("Failed to stop accessory scan on navigation: {e}");
                }
            });
        }
    }
}

/// Aborts the tokio scan loop when the page's task is dropped.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Keeps the view's `scan` up to date with the DryFireMag and MantisX
/// accessories in range, or with why it cannot look for them. Scanning stops
/// when the returned task is dropped.
pub(super) fn follow(cx: &mut Context<'_, AccessoriesView>) -> Task<()> {
    let (updates, mut results) = mpsc::unbounded_channel();
    let scanner = AbortOnDrop(runtime::spawn(scan_loop(updates)));
    cx.spawn(async move |this, cx| {
        let _scanner = scanner;
        while let Some(scan) = results.recv().await {
            let applied = this.update(cx, |this, cx| {
                this.scan = scan;
                cx.notify();
            });
            if applied.is_err() {
                break;
            }
        }
    })
}

async fn scan_loop(updates: mpsc::UnboundedSender<BleScan>) {
    let manager = match Manager::new().await {
        Ok(manager) => manager,
        Err(error) => {
            tracing::warn!("Bluetooth unavailable: {error}");
            let _ = updates.send(BleScan::Unavailable(error.to_string()));
            return;
        }
    };
    let adapters = match manager.adapters().await {
        Ok(adapters) => adapters,
        Err(error) => {
            tracing::warn!("Bluetooth unavailable: {error}");
            let _ = updates.send(BleScan::Unavailable(error.to_string()));
            return;
        }
    };
    let Some(adapter) = adapters.into_iter().next() else {
        let _ = updates.send(BleScan::Unavailable(
            "this computer has no Bluetooth adapter".to_string(),
        ));
        return;
    };
    let mut session = ScanSession {
        adapter,
        active: false,
    };
    loop {
        let found = scan_once(&mut session).await;
        if updates.send(BleScan::Found(found)).is_err() {
            return;
        }
        tokio::time::sleep(SCAN_PAUSE).await;
    }
}

async fn scan_once(session: &mut ScanSession) -> Nearby {
    let filter = ScanFilter { services: vec![] };
    // Mark active before awaiting so cancellation during start also cleans up.
    session.active = true;
    if let Err(e) = session.adapter.start_scan(filter).await {
        tracing::warn!("Failed to start accessory scan: {e}");
    }
    tokio::time::sleep(SCAN_DURATION).await;
    match session.adapter.stop_scan().await {
        Ok(()) => session.active = false,
        Err(e) => tracing::warn!("Failed to stop accessory scan: {e}"),
    }

    let mut found = Nearby::new();
    let Ok(peripherals) = session.adapter.peripherals().await else {
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
