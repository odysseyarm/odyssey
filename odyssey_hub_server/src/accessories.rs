use btleplug::api::{Manager as _, Central, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use odyssey_hub_common::AccessoryInfo;
use uuid::Uuid;
use std::{collections::HashMap, collections::HashSet, sync::Arc, time::Duration};
use tokio::{sync::mpsc::Sender, time};
use odyssey_hub_common::events::Event;

// #[derive(Debug, Clone)]
// pub enum Message {
//     Connect(
//         [u8; 6],
//         Sender<AccessoryMessage>,
//     ),
//     Disconnect([u8; 6]),
//     Event(odyssey_hub_common::events::Event),
// }

pub async fn accessory_scanner(
    mut event_tx: Sender<Event>,
    device_list: Arc<
        parking_lot::Mutex<
            Vec<(
                odyssey_hub_common::device::Device,
                ats_usb::device::UsbDevice,
                tokio::sync::mpsc::Sender<crate::device_tasks::DeviceTaskMessage>,
            )>,
        >
    >,
) {
    let nus_uuid = Uuid::from_bytes([0x6e,0x40,0x00,0x01, 0xb5,0xa3, 0xf3,0x93, 0xe0,0xa9, 0xe5,0x0e,0x24,0xdc,0xca,0x9e]);
    let manager = Manager::new().await.unwrap();
    let adapter = loop {
        let adapter = manager.adapters().await.unwrap()
        .into_iter()
        .next();
        if let Some(adapter) = adapter {
            break adapter;
        }
        time::sleep(Duration::from_secs(1)).await;
    };

    let filter = ScanFilter { services: vec![nus_uuid] };
    let mut known = HashMap::new();

    loop {
        adapter.start_scan(filter.clone()).await.unwrap();
        time::sleep(Duration::from_secs(3)).await;
        adapter.stop_scan().await.unwrap();

        let mut seen = HashSet::new();
        for p in adapter.peripherals().await.unwrap() {
            if let Some(props) = p.properties().await.unwrap() {
                let name = props.local_name.unwrap_or_default();
                if name != "DryFireMag" {
                    continue;
                }
                let uuid = props.address.into_inner();
                seen.insert(uuid);
                // todo set assignment from map
                let accessory = AccessoryInfo { name, ty: odyssey_hub_common::AccessoryType::DryFireMag, assignment: None };
                known.insert(uuid, accessory.clone());
            }
        }

        for removed in known.clone().into_keys().collect::<HashSet<_>>().difference(&seen) {
            known.remove(removed);
        }

        time::sleep(Duration::from_secs(5)).await;
    }
}
