use dioxus::prelude::*;
use futures::StreamExt;
use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU64,
    time::Duration,
};

use crate::hub; // HubContext
use odyssey_hub_common::accessory::{AccessoryInfo, AccessoryMap, AccessoryType};

// UI-side BLE scan (btleplug)
use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use uuid::Uuid;

#[derive(Props, PartialEq, Clone)]
struct StatusChipProps {
    connected: bool,
}

#[component]
fn StatusChip(props: StatusChipProps) -> Element {
    let (label, dot_cls, pill_cls) = if props.connected {
        (
            "Connected",
            "h-2.5 w-2.5 rounded-full bg-emerald-500 animate-pulse",
            "bg-emerald-50 text-emerald-700 border-emerald-200",
        )
    } else {
        (
            "Disconnected",
            "h-2.5 w-2.5 rounded-full bg-gray-400",
            "bg-gray-50 text-gray-700 border-gray-200",
        )
    };

    rsx! {
        span { class: "inline-flex items-center gap-2 px-2.5 py-1 rounded-full text-xs border {pill_cls}",
            span { class: "{dot_cls}" }
            "{label}"
        }
    }
}

#[inline]
fn slot_color_dot(slot: usize) -> &'static str {
    match slot {
        0 => "bg-red-500",
        1 => "bg-blue-500",
        2 => "bg-yellow-400",
        3 => "bg-green-500",
        4 => "bg-orange-500",
        5 => "bg-gray-400",
        _ => "bg-gray-400",
    }
}

#[component]
pub fn Accessories() -> Element {
    let hub = use_context::<Signal<hub::HubContext>>();

    // id -> (info, connected)
    let accessory_map = use_signal(|| HashMap::<[u8; 6], (AccessoryInfo, bool)>::new());
    // id -> info
    let info_map = use_signal(|| HashMap::<[u8; 6], AccessoryInfo>::new());

    // Nearby discovered by BLE: id -> (label, type)
    let nearby = use_signal(|| HashMap::<[u8; 6], (String, AccessoryType)>::new());

    let format_mac = |mac: &[u8; 6]| -> String {
        mac.iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(":")
    };

    // Seed + subscribe AccessoryMap
    use_future({
        let hub_sig = hub.clone();
        let mut accessory_map = accessory_map.clone();
        let mut info_map = info_map.clone();
        move || async move {
            let mut client = hub_sig.read().client.read().clone();

            if let Ok(m) = client.get_accessory_map().await {
                accessory_map.set(m.clone());
                let infos = m
                    .iter()
                    .map(|(id, (info, _))| (*id, info.clone()))
                    .collect();
                info_map.set(infos);
            }

            if let Ok(mut stream) = client.subscribe_accessory_map().await {
                while let Some(Ok(next)) = stream.next().await {
                    let m: AccessoryMap = next;
                    accessory_map.set(m.clone());
                    let infos = m
                        .iter()
                        .map(|(id, (info, _))| (*id, info.clone()))
                        .collect();
                    info_map.set(infos);
                }
            }
        }
    });

    // BLE scan loop: detect DryFireMag and BlackbeardX
    use_future({
        let mut nearby = nearby.clone();
        move || async move {
            let _nus_uuid = Uuid::from_bytes([
                0x6e, 0x40, 0x00, 0x01, 0xb5, 0xa3, 0xf3, 0x93, 0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc,
                0xca, 0x9e,
            ]);
            let blackbeardx_uuid = Uuid::from_bytes([
                0x6e, 0x40, 0x00, 0x01, 0x20, 0x4d, 0x61, 0x6e, 0x74, 0x69, 0x73, 0x20, 0x54, 0x65,
                0x63, 0x68,
            ]);

            let Ok(manager) = Manager::new().await else {
                return;
            };
            let Ok(mut adapters) = manager.adapters().await else {
                return;
            };
            let Some(adapter) = adapters.drain(..).next() else {
                return;
            };

            let filter = ScanFilter { services: vec![] };

            loop {
                let _ = adapter.start_scan(filter.clone()).await;
                tokio::time::sleep(Duration::from_secs(3)).await;
                let _ = adapter.stop_scan().await;

                let mut found = HashMap::<[u8; 6], (String, AccessoryType)>::new();
                if let Ok(peris) = adapter.peripherals().await {
                    for p in peris {
                        if let Ok(Some(props)) = p.properties().await {
                            let id = props.address.into_inner();
                            if props.local_name.as_deref() == Some("DryFireMag") {
                                found.insert(
                                    id,
                                    ("DryFireMag".to_string(), AccessoryType::DryFireMag),
                                );
                            } else if props.services.contains(&blackbeardx_uuid) {
                                found.insert(
                                    id,
                                    ("BlackbeardX".to_string(), AccessoryType::BlackbeardX),
                                );
                            }
                        }
                    }
                }
                nearby.set(found);

                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    });

    // add: ([u8;6], String, AccessoryType)
    let add = {
        let hub_sig = hub.clone();
        let info0 = info_map.clone();
        use_callback(move |(id, name, ty): ([u8; 6], String, AccessoryType)| {
            let next: HashMap<[u8; 6], AccessoryInfo> = {
                let cur = info0.read();
                let mut m = cur.clone();
                drop(cur);
                m.insert(id, AccessoryInfo::with_defaults(name, ty));
                m
            };

            let mut client = hub_sig.read().client.read().clone();
            let mut info_map = info0.clone();
            spawn(async move {
                let _ = client.update_accessory_info_map(next.clone()).await;
                info_map.set(next);
            });
        })
    };

    // remove: ([u8;6])
    let remove = {
        let hub_sig = hub.clone();
        let info0 = info_map.clone();
        use_callback(move |id: [u8; 6]| {
            let next: HashMap<[u8; 6], AccessoryInfo> = {
                let cur = info0.read();
                let mut m = cur.clone();
                drop(cur);
                m.remove(&id);
                m
            };

            let mut client = hub_sig.read().client.read().clone();
            let mut info_map = info0.clone();
            spawn(async move {
                let _ = client.update_accessory_info_map(next.clone()).await;
                info_map.set(next);
            });
        })
    };

    // assign: ([u8;6], Option<NonZeroU64>) — UUID-based
    let assign = {
        let hub_sig = hub.clone();
        let info0 = info_map.clone();
        use_callback(move |(id, assign_uuid): ([u8; 6], Option<NonZeroU64>)| {
            let next: HashMap<[u8; 6], AccessoryInfo> = {
                let cur = info0.read();
                let mut m = cur.clone();
                drop(cur);
                if let Some(info) = m.get_mut(&id) {
                    info.assignment = assign_uuid;
                }
                m
            };

            let mut client = hub_sig.read().client.read().clone();
            let mut info_map = info0.clone();
            spawn(async move {
                let _ = client.update_accessory_info_map(next.clone()).await;
                info_map.set(next);
            });
        })
    };

    // Snapshots for render
    let info_snapshot: HashMap<[u8; 6], AccessoryInfo> = { info_map.read().clone() };
    let am_snapshot: HashMap<[u8; 6], (AccessoryInfo, bool)> = { accessory_map.read().clone() };
    let mut added_ids: Vec<[u8; 6]> = info_snapshot.keys().cloned().collect();

    // Devices (for assignment)
    let devices_snapshot = (hub().devices)();
    let device_choices: Vec<(NonZeroU64, String, usize)> = devices_snapshot
        .iter()
        .filter_map(|(slot, dev)| {
            let mut buf = [0u8; 8];
            buf[..6].copy_from_slice(&dev.uuid);
            let uuid_u64 = u64::from_le_bytes(buf);
            let nz = NonZeroU64::new(uuid_u64)?;
            let label = format!("0x{:x}", uuid_u64);
            Some((nz, label, slot))
        })
        .collect();

    // Sort connected first, then MAC
    added_ids.sort_by_key(|id| {
        let connected = am_snapshot.get(id).map(|(_, c)| *c).unwrap_or(false);
        (if connected { 0 } else { 1 }, *id)
    });

    let connected_count = am_snapshot.values().filter(|(_, c)| *c).count();
    let total_count = added_ids.len();

    let nearby_snapshot: HashMap<[u8; 6], (String, AccessoryType)> = { nearby.read().clone() };
    let mut nearby_addable: Vec<([u8; 6], (String, AccessoryType))> = nearby_snapshot
        .iter()
        .filter(|(id, _)| !info_snapshot.contains_key(*id))
        .map(|(id, v)| (*id, v.clone()))
        .collect();
    nearby_addable.sort_by_key(|(id, _)| *id);

    rsx! {
        div { class: "p-4 space-y-6",

            section { class: "space-y-2",
                h2 { class: "text-lg font-semibold flex items-center gap-2",
                    "Accessories"
                    span { class: "text-xs font-normal opacity-70",
                        "({connected_count}/{total_count} connected)"
                    }
                }
                if added_ids.is_empty() {
                    p { class: "text-sm opacity-70", "No accessories added yet." }
                } else {
                    ul { class: "divide-y divide-gray-200/40 dark:divide-white/10 rounded-xl overflow-hidden shadow-sm",
                        {added_ids.iter().map(|id| {
                            let (info, connected) = am_snapshot.get(id)
                                .cloned()
                                .unwrap_or_else(|| {
                                    (
                                        info_snapshot.get(id).cloned().unwrap_or_else(|| AccessoryInfo::with_defaults("Unknown", AccessoryType::DryFireMag)),
                                        false
                                    )
                                });

                            let mac = format_mac(id);
                            let assigned_uuid = info.assignment.map(|nz| nz.get());

                            let (assigned_slot_opt, assigned_label) = if let Some(uuid_v) = assigned_uuid {
                                if let Some((_uuid_nz, label, slot)) = device_choices
                                    .iter()
                                    .find(|(nz, _, _)| nz.get() == uuid_v) {
                                    (Some(*slot), label.clone())
                                } else {
                                    let offline_label = format!("0x{:x} (offline)", uuid_v);
                                    (None, offline_label)
                                }
                            } else {
                                (None, "(Unassigned)".to_string())
                            };

                            rsx! {
                                li { class: "flex flex-col gap-2 md:flex-row md:items-center md:justify-between px-4 py-3 bg-white/70 dark:bg-zinc-900/60 backdrop-blur",

                                    div { class: "min-w-0",
                                        div { class: "font-medium truncate", "{info.name}" }
                                        div { class: "text-xs opacity-70 truncate", "{mac}" }
                                    }

                                    div { class: "flex items-center gap-3 flex-wrap justify-end",
                                        StatusChip { connected }

                                        {
                                            let dot_class = slot_color_dot(assigned_slot_opt.unwrap_or(5));
                                            rsx!{
                                                span { class: "inline-flex items-center gap-2 text-xs px-2 py-1 rounded-full border border-gray-200/70 dark:border-white/10",
                                                    span { class: "h-2.5 w-2.5 rounded-full {dot_class}" }
                                                    "{assigned_label}"
                                                }
                                            }
                                        }

                                        select {
                                            class: "px-2 py-1 text-sm rounded-lg border border-gray-300/70 dark:border-white/10 bg-white/60 dark:bg-zinc-900/60",
                                            value: assigned_uuid.map(|v| v.to_string()).unwrap_or_default(),
                                            onchange: {
                                                let id = *id;
                                                let assign = assign.clone();
                                                move |evt| {
                                                    let s = evt.value();
                                                    if s.is_empty() {
                                                        assign.call((id, None));
                                                    } else if let Ok(v) = s.parse::<u64>() {
                                                        if let Some(nz) = NonZeroU64::new(v) {
                                                            assign.call((id, Some(nz)));
                                                        }
                                                    }
                                                }
                                            },
                                            option { value: "", "(Unassigned)" }
                                            {device_choices.iter().map(|(nz, label, slot)| {
                                                let display = format!("{label} (slot {})", slot + 1);
                                                rsx!{
                                                    option { value: "{nz.get()}", "{display}" }
                                                }
                                            })}
                                        }

                                        button {
                                            class: "px-3 py-1.5 text-sm rounded-lg border border-red-500/60 hover:bg-red-500 hover:text-white transition",
                                            onclick: {
                                                let id = *id;
                                                let remove = remove.clone();
                                                move |_| remove.call(id)
                                            },
                                            "Remove"
                                        }
                                    }
                                }
                            }
                        })}
                    }
                }
            }

            section { class: "space-y-2",
                h2 { class: "text-lg font-semibold", "Nearby" }
                if nearby_addable.is_empty() {
                    p { class: "text-sm opacity-70", "No nearby devices (or all are already added)." }
                } else {
                    ul { class: "divide-y divide-gray-200/40 dark:divide-white/10 rounded-xl overflow-hidden shadow-sm",
                        {nearby_addable.iter().map(|(id, (label, ty))| {
                            let mac = format_mac(id);
                            rsx! {
                                li { class: "flex items-center justify-between px-4 py-3 bg-white/70 dark:bg-zinc-900/60 backdrop-blur",
                                    div { class: "min-w-0",
                                        div { class: "font-medium truncate", "{label}" }
                                        div { class: "text-xs opacity-70 truncate", "{mac}" }
                                    }
                                    button {
                                        class: "px-3 py-1.5 text-sm rounded-lg border border-emerald-600/60 hover:bg-emerald-600 hover:text-white transition",
                                        onclick: {
                                            let id = *id;
                                            let label = label.clone();
                                            let ty = *ty;
                                            let add = add.clone();
                                            move |_| add.call((id, label.clone(), ty))
                                        },
                                        "Add"
                                    }
                                }
                            }
                        })}
                    }
                }
            }
        }
    }
}
