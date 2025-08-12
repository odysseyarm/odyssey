use dioxus::prelude::*;
use futures::StreamExt;
use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU64,
    time::Duration,
};

use crate::hub; // HubContext
use odyssey_hub_common::{AccessoryInfo, AccessoryMap, AccessoryType};

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
    // red, blue, yellow, green, orange, gray (match CROSSHAIRS order)
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
    // Provided as Signal<HubContext> in app()
    let hub = use_context::<Signal<hub::HubContext>>();

    // Full map from server: id -> (info, connected)
    let accessory_map = use_signal(|| HashMap::<[u8; 6], (AccessoryInfo, bool)>::new());

    // Local editable info map (what we send via update_accessory_info_map)
    let info_map = use_signal(|| HashMap::<[u8; 6], AccessoryInfo>::new());

    // Nearby devices discovered by local BLE (potential candidates to Add)
    let nearby = use_signal(|| HashSet::<[u8; 6]>::new());

    let format_mac = |mac: &[u8; 6]| -> String {
        mac.iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(":")
    };

    // ---------- seed current state + subscribe live ----------
    use_future({
        let hub_sig = hub.clone();
        let mut accessory_map = accessory_map.clone();
        let mut info_map = info_map.clone();
        move || async move {
            // Clone an owned Client using only read locks
            let mut client = hub_sig.read().client.read().clone();

            if let Ok(m) = client.get_accessory_map().await {
                accessory_map.set(m.clone());
                let info_snapshot: HashMap<[u8; 6], AccessoryInfo> = m
                    .iter()
                    .map(|(id, (info, _))| (*id, info.clone()))
                    .collect();
                info_map.set(info_snapshot);
            }

            if let Ok(mut stream) = client.subscribe_accessory_map().await {
                while let Some(Ok(next)) = stream.next().await {
                    let m: AccessoryMap = next;
                    accessory_map.set(m.clone());
                    let info_snapshot: HashMap<[u8; 6], AccessoryInfo> = m
                        .iter()
                        .map(|(id, (info, _))| (*id, info.clone()))
                        .collect();
                    info_map.set(info_snapshot);
                }
            }
        }
    });

    // ---------- UI-side BLE scan loop (find things to Add) ----------
    use_future({
        let mut nearby = nearby.clone();
        move || async move {
            // Keep UUID for reference, but don't filter by it (Windows often omits service UUIDs in passive scans)
            let _nus_uuid = Uuid::from_bytes([
                0x6e, 0x40, 0x00, 0x01, 0xb5, 0xa3, 0xf3, 0x93, 0xe0, 0xa9, 0xe5, 0x0e, 0x24, 0xdc,
                0xca, 0x9e,
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

            // Windows-friendly: empty filter
            let filter = ScanFilter { services: vec![] };

            loop {
                let _ = adapter.start_scan(filter.clone()).await;
                tokio::time::sleep(Duration::from_secs(3)).await;
                let _ = adapter.stop_scan().await;

                let mut found = HashSet::<[u8; 6]>::new();
                if let Ok(peris) = adapter.peripherals().await {
                    for p in peris {
                        if let Ok(Some(props)) = p.properties().await {
                            if props.local_name.as_deref() == Some("DryFireMag") {
                                found.insert(props.address.into_inner());
                            }
                        }
                    }
                }
                nearby.set(found);

                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    });

    // ---------- Actions via use_callback (clone Client from read locks only) ----------
    // add: takes ([u8;6], String)
    let add = {
        let hub_sig = hub.clone();
        let info0 = info_map.clone();
        use_callback(move |(id, name): ([u8; 6], String)| {
            // Build "next" without holding a live read across the await
            let next: HashMap<[u8; 6], AccessoryInfo> = {
                let cur = info0.read();
                let mut m = cur.clone();
                drop(cur);
                m.insert(
                    id,
                    AccessoryInfo {
                        name,
                        ty: AccessoryType::DryFireMag,
                        assignment: None::<NonZeroU64>,
                    },
                );
                m
            };

            let mut client = hub_sig.read().client.read().clone();
            let mut info_map = info0.clone();

            spawn(async move {
                let _ = client.update_accessory_info_map(next.clone()).await;
                // Optimistic local update; server stream will reconcile
                info_map.set(next);
            });
        })
    };

    // remove: takes ([u8;6])
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

    // assign: takes ([u8;6], Option<NonZeroU64>) — sets or clears assignment (UUID-based)
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

    // ---------- derive snapshots for render (no live reads in rsx) ----------
    let info_snapshot: HashMap<[u8; 6], AccessoryInfo> = { info_map.read().clone() };
    let am_snapshot: HashMap<[u8; 6], (AccessoryInfo, bool)> = { accessory_map.read().clone() };
    let mut added_ids: Vec<[u8; 6]> = info_snapshot.keys().cloned().collect();

    // Live device list from HubContext (reactive). Expect (hub().devices)() => iterable of (slot, device) where device.uuid() -> u64
    let devices_snapshot = (hub().devices)();

    // Build dropdown choices: (uuid_nz, label, slot)
    let device_choices: Vec<(NonZeroU64, String, usize)> = devices_snapshot
        .iter()
        .filter_map(|(slot, dev)| {
            let uuid_u64 = dev.uuid();
            let nz = NonZeroU64::new(uuid_u64)?;
            let label = format!("0x{:x}", uuid_u64);
            Some((nz, label, slot))
        })
        .collect();

    // Sort: connected first, then by MAC for stability
    added_ids.sort_by_key(|id| {
        let connected = am_snapshot.get(id).map(|(_, c)| *c).unwrap_or(false);
        (if connected { 0 } else { 1 }, *id)
    });

    let connected_count = am_snapshot.values().filter(|(_, c)| *c).count();
    let total_count = added_ids.len();

    let nearby_snapshot: HashSet<[u8; 6]> = { nearby.read().clone() };
    let nearby_addable: Vec<[u8; 6]> = nearby_snapshot
        .iter()
        .filter(|id| !info_snapshot.contains_key(*id))
        .cloned()
        .collect();

    rsx! {
        div { class: "p-4 space-y-6",

            // Persisted accessories (with live connected status)
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
                                        info_snapshot.get(id).cloned().unwrap_or(AccessoryInfo {
                                            name: "Unknown".into(),
                                            ty: AccessoryType::DryFireMag,
                                            assignment: None::<NonZeroU64>,
                                        }),
                                        false
                                    )
                                });

                            let mac = format_mac(id);

                            // Current assignment UUID (u64)
                            let assigned_uuid = info.assignment.map(|nz| nz.get());

                            // Find live device by UUID to get slot (for color), otherwise mark offline
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

                                    // Left: name + MAC
                                    div { class: "min-w-0",
                                        div { class: "font-medium truncate", "{info.name}" }
                                        div { class: "text-xs opacity-70 truncate", "{mac}" }
                                    }

                                    // Right: status + assignment dropdown + remove
                                    div { class: "flex items-center gap-3 flex-wrap justify-end",

                                        // Connected status pill
                                        StatusChip { connected }

                                        // Assignment chip (color + uuid/label)
                                        {
                                            let dot_class = slot_color_dot(assigned_slot_opt.unwrap_or(5)); // gray when None/offline
                                            rsx!{
                                                span { class: "inline-flex items-center gap-2 text-xs px-2 py-1 rounded-full border border-gray-200/70 dark:border-white/10",
                                                    span { class: "h-2.5 w-2.5 rounded-full {dot_class}" }
                                                    "{assigned_label}"
                                                }
                                            }
                                        }

                                        // Assignment dropdown (UUID values; stays selected even if device goes offline)
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

                                        // Remove accessory
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

            // Nearby discovered devices (not yet added)
            section { class: "space-y-2",
                h2 { class: "text-lg font-semibold", "Nearby" }
                if nearby_addable.is_empty() {
                    p { class: "text-sm opacity-70", "No nearby devices (or all are already added)." }
                } else {
                    ul { class: "divide-y divide-gray-200/40 dark:divide-white/10 rounded-xl overflow-hidden shadow-sm",
                        {nearby_addable.iter().map(|id| {
                            let mac = format_mac(id);
                            rsx! {
                                li { class: "flex items-center justify-between px-4 py-3 bg-white/70 dark:bg-zinc-900/60 backdrop-blur",
                                    div { class: "min-w-0",
                                        div { class: "font-medium truncate", "DryFireMag" }
                                        div { class: "text-xs opacity-70 truncate", "{mac}" }
                                    }
                                    button {
                                        class: "px-3 py-1.5 text-sm rounded-lg border border-emerald-600/60 hover:bg-emerald-600 hover:text-white transition",
                                        onclick: {
                                            let id = *id;
                                            let add = add.clone();
                                            move |_| add.call((id, "DryFireMag".to_string()))
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
