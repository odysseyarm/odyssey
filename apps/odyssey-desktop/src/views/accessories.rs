//! The Accessories page: the accessories the hub knows, and the ones in range
//! that can be added.

mod ble_scan;
mod edits;
mod hub_sync;
mod status_chip;

use std::num::NonZeroU64;

use ble_scan::{Nearby, use_ble_scan};
use dioxus::prelude::*;
use edits::AccessoryEdits;
use hub_sync::{Known, use_accessory_map};
use odyssey_hub_common::accessory::{AccessoryInfo, AccessoryInfoMap, AccessoryType};
use status_chip::StatusChip;

use crate::hub::HubContext;
use crate::views::device_label::format_address;

/// The slot color shown for an accessory whose device is unknown.
const UNASSIGNED_SLOT: usize = 5;

fn slot_color_dot(slot: usize) -> &'static str {
    match slot {
        0 => "bg-red-500",
        1 => "bg-blue-500",
        2 => "bg-yellow-400",
        3 => "bg-green-500",
        4 => "bg-orange-500",
        _ => "bg-gray-400",
    }
}

/// What the assignment dropdown reports: `None` for a value that is not a
/// choice, `Some(None)` for "(Unassigned)", `Some(Some(uuid))` for a device.
fn parse_assignment(selected: &str) -> Option<Option<NonZeroU64>> {
    if selected.is_empty() {
        return Some(None);
    }
    let uuid = selected.parse::<u64>().ok()?;
    NonZeroU64::new(uuid).map(Some)
}

#[component]
pub fn Accessories() -> Element {
    let hub = use_context::<Signal<HubContext>>();

    let known = use_signal(Known::new);
    let infos = use_signal(AccessoryInfoMap::new);
    let nearby = use_signal(Nearby::new);

    use_accessory_map(hub, known, infos);
    use_ble_scan(nearby);
    let edits = AccessoryEdits::new(hub, infos);

    // Snapshots for render
    let info_snapshot = infos.read().clone();
    let known_snapshot = known.read().clone();
    let is_connected = |id: &[u8; 6]| {
        known_snapshot
            .get(id)
            .is_some_and(|(_, connected)| *connected)
    };

    // Connected devices an accessory can be assigned to: the uuid the hub
    // matches on, how to label it, and its slot.
    let device_choices: Vec<(NonZeroU64, String, usize)> = (hub().devices)()
        .iter()
        .filter_map(|(slot, device)| {
            let mut bytes = [0u8; 8];
            bytes[..6].copy_from_slice(&device.uuid);
            let uuid_u64 = u64::from_le_bytes(bytes);
            Some((NonZeroU64::new(uuid_u64)?, format!("0x{uuid_u64:x}"), slot))
        })
        .collect();

    // Connected first, then by MAC.
    let mut added_ids: Vec<[u8; 6]> = info_snapshot.keys().copied().collect();
    added_ids.sort_by_key(|id| (!is_connected(id), *id));

    let connected_count = known_snapshot
        .values()
        .filter(|(_, connected)| *connected)
        .count();
    let total_count = added_ids.len();

    let mut nearby_addable: Vec<([u8; 6], (String, AccessoryType))> = nearby
        .read()
        .iter()
        .filter(|(id, _)| !info_snapshot.contains_key(*id))
        .map(|(id, found)| (*id, found.clone()))
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
                        for id in added_ids {
                            AddedAccessory {
                                key: "{format_address(&id)}",
                                id,
                                info: added_info(&known_snapshot, &info_snapshot, &id),
                                connected: is_connected(&id),
                                device_choices: device_choices.clone(),
                                edits,
                            }
                        }
                    }
                }
            }

            section { class: "space-y-2",
                h2 { class: "text-lg font-semibold", "Nearby" }
                if nearby_addable.is_empty() {
                    p { class: "text-sm opacity-70", "No nearby devices (or all are already added)." }
                } else {
                    ul { class: "divide-y divide-gray-200/40 dark:divide-white/10 rounded-xl overflow-hidden shadow-sm",
                        for (id, (label, ty)) in nearby_addable {
                            NearbyAccessory { key: "{format_address(&id)}", id, label, ty, edits }
                        }
                    }
                }
            }
        }
    }
}

/// What to show for an added accessory: what the hub reports, else what was
/// last saved, else a placeholder.
fn added_info(known: &Known, saved: &AccessoryInfoMap, id: &[u8; 6]) -> AccessoryInfo {
    if let Some((info, _)) = known.get(id) {
        return info.clone();
    }
    saved
        .get(id)
        .cloned()
        .unwrap_or_else(|| AccessoryInfo::with_defaults("Unknown", AccessoryType::DryFireMag))
}

#[component]
fn AddedAccessory(
    id: [u8; 6],
    info: AccessoryInfo,
    connected: bool,
    device_choices: Vec<(NonZeroU64, String, usize)>,
    edits: AccessoryEdits,
) -> Element {
    let mac = format_address(&id);
    let assigned_uuid = info.assignment.map(NonZeroU64::get);

    let (assigned_slot, assigned_label) = match assigned_uuid {
        None => (None, "(Unassigned)".to_string()),
        Some(uuid) => match device_choices
            .iter()
            .find(|(choice_uuid, _, _)| choice_uuid.get() == uuid)
        {
            Some((_, label, slot)) => (Some(*slot), label.clone()),
            None => (None, format!("0x{uuid:x} (offline)")),
        },
    };
    let dot_class = slot_color_dot(assigned_slot.unwrap_or(UNASSIGNED_SLOT));

    rsx! {
        li { class: "flex flex-col gap-2 md:flex-row md:items-center md:justify-between px-4 py-3 bg-white/70 dark:bg-zinc-900/60 backdrop-blur",

            div { class: "min-w-0",
                div { class: "font-medium truncate", "{info.name}" }
                div { class: "text-xs opacity-70 truncate", {mac} }
            }

            div { class: "flex items-center gap-3 flex-wrap justify-end",
                StatusChip { connected }

                span { class: "inline-flex items-center gap-2 text-xs px-2 py-1 rounded-full border border-gray-200/70 dark:border-white/10",
                    span { class: "h-2.5 w-2.5 rounded-full {dot_class}" }
                    {assigned_label}
                }

                select {
                    class: "px-2 py-1 text-sm rounded-lg border border-gray-300/70 dark:border-white/10 bg-white/60 dark:bg-zinc-900/60",
                    value: assigned_uuid.map(|uuid| uuid.to_string()).unwrap_or_default(),
                    onchange: move |evt| {
                        if let Some(assignment) = parse_assignment(&evt.value()) {
                            edits.assign(id, assignment);
                        }
                    },
                    option { value: "", "(Unassigned)" }
                    for (uuid, label, slot) in device_choices {
                        option { key: "{uuid}", value: "{uuid}", "{label} (slot {slot + 1})" }
                    }
                }

                button {
                    class: "px-3 py-1.5 text-sm rounded-lg border border-red-500/60 hover:bg-red-500 hover:text-white transition",
                    onclick: move |_| edits.remove(id),
                    "Remove"
                }
            }
        }
    }
}

#[component]
fn NearbyAccessory(
    id: [u8; 6],
    label: String,
    ty: AccessoryType,
    edits: AccessoryEdits,
) -> Element {
    let mac = format_address(&id);
    rsx! {
        li { class: "flex items-center justify-between px-4 py-3 bg-white/70 dark:bg-zinc-900/60 backdrop-blur",
            div { class: "min-w-0",
                div { class: "font-medium truncate", {label.as_str()} }
                div { class: "text-xs opacity-70 truncate", {mac} }
            }
            button {
                class: "px-3 py-1.5 text-sm rounded-lg border border-emerald-600/60 hover:bg-emerald-600 hover:text-white transition",
                onclick: move |_| edits.add(id, label.clone(), ty),
                "Add"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dropdown_value_maps_to_an_assignment() {
        assert_eq!(parse_assignment(""), Some(None));
        assert_eq!(parse_assignment("42"), Some(NonZeroU64::new(42)));
        // Zero is not a device and text is not a choice: the edit is ignored.
        assert_eq!(parse_assignment("0"), None);
        assert_eq!(parse_assignment("abc"), None);
    }
}
