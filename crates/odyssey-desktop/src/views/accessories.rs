//! The Accessories page: the accessories the hub knows, and the ones in range
//! that can be added.

mod ble_scan;
mod edits;
mod hub_sync;
mod status_chip;

use std::num::NonZeroU64;

use ble_scan::{BleScan, use_ble_scan};
use dioxus::prelude::*;
use edits::AccessoryEdits;
use hub_sync::{Known, use_accessory_map};
use odyssey_hub_common::accessory::{AccessoryInfo, AccessoryInfoMap, AccessoryType};
use status_chip::StatusChip;

use crate::components::Page;
use crate::hub::HubContext;
use crate::views::device_label;
use crate::views::device_label::format_address;

/// The slot color shown for an accessory whose device is unknown.
const UNASSIGNED_SLOT: usize = 5;

const LIST_CLASS: &str = "divide-y divide-line";
const EMPTY_CLASS: &str = "text-sm text-fg-muted";

fn slot_color_dot(slot: usize) -> &'static str {
    match slot {
        0 => "bg-danger",
        1 => "bg-info",
        2 => "bg-warning",
        3 => "bg-success",
        4 => "bg-accent",
        _ => "bg-fg-faint",
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
    let scan = use_signal(|| BleScan::Starting);

    use_accessory_map(hub, known, infos);
    use_ble_scan(scan);
    let edits = AccessoryEdits::use_new(hub, infos);

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
            Some((NonZeroU64::new(uuid_u64)?, device_label(device), slot))
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

    let nearby = match &*scan.read() {
        BleScan::Found(found) => {
            let mut addable: Vec<([u8; 6], (String, AccessoryType))> = found
                .iter()
                .filter(|(id, _)| !info_snapshot.contains_key(*id))
                .map(|(id, found)| (*id, found.clone()))
                .collect();
            addable.sort_by_key(|(id, _)| *id);
            Ok(addable)
        }
        BleScan::Starting => Err(None),
        BleScan::Unavailable(reason) => Err(Some(reason.clone())),
    };

    rsx! {
        Page {
            title: "Accessories",
            description: "Bluetooth accessories such as a DryFireMag or MantisX. Assign one to a device to pair their events.",

            section { class: "card",
                h2 { class: "card-title flex items-baseline gap-2",
                    "Added"
                    span { class: "text-sm font-normal text-fg-muted",
                        "{connected_count} of {total_count} connected"
                    }
                }
                if added_ids.is_empty() {
                    p { class: "{EMPTY_CLASS} mt-1", "None yet. Add one from the nearby list below." }
                } else {
                    ul { class: LIST_CLASS,
                        for id in added_ids {
                            AddedAccessory {
                                key: "{format_address(&id)}",
                                id,
                                info: info_snapshot[&id].clone(),
                                connected: is_connected(&id),
                                device_choices: device_choices.clone(),
                                edits,
                            }
                        }
                    }
                }
            }

            section { class: "card",
                h2 { class: "card-title", "Nearby" }
                match nearby {
                    Err(None) => rsx! {
                        p { class: "{EMPTY_CLASS} mt-1", "Looking for accessories…" }
                    },
                    Err(Some(reason)) => rsx! {
                        p {
                            class: "mt-1 text-sm text-warning-ink",
                            role: "status",
                            "Bluetooth is unavailable, so nearby accessories cannot be found: {reason}."
                        }
                    },
                    Ok(addable) if addable.is_empty() => rsx! {
                        p { class: "{EMPTY_CLASS} mt-1",
                            "No new accessories in range. Switch one on and keep it near this computer; the list refreshes every few seconds."
                        }
                    },
                    Ok(addable) => rsx! {
                        ul { class: LIST_CLASS,
                            for (id, (label, ty)) in addable {
                                NearbyAccessory { key: "{format_address(&id)}", id, label, ty, edits }
                            }
                        }
                    },
                }
            }
        }
    }
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
        None => (None, "Unassigned".to_string()),
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
        li { class: "flex flex-col gap-2 md:flex-row md:items-center md:justify-between py-3",

            div { class: "min-w-0",
                div { class: "font-medium text-fg-strong truncate", "{info.name}" }
                div { class: "text-xs font-mono text-fg-muted truncate", {mac} }
            }

            div { class: "flex items-center gap-3 flex-wrap justify-end",
                StatusChip { connected }

                span { class: "badge badge-neutral gap-2",
                    span { class: "size-2 rounded-full {dot_class}", aria_hidden: true }
                    {assigned_label}
                }

                select {
                    class: "field px-2 py-1",
                    aria_label: "Device for {info.name}",
                    disabled: edits.pending(),
                    value: assigned_uuid.map(|uuid| uuid.to_string()).unwrap_or_default(),
                    onchange: move |evt| {
                        if let Some(assignment) = parse_assignment(&evt.value()) {
                            edits.assign(id, assignment);
                        }
                    },
                    option { value: "", "Unassigned" }
                    for (uuid, label, slot) in device_choices {
                        option { key: "{uuid}", value: "{uuid}", "{label} (slot {slot + 1})" }
                    }
                }

                button {
                    class: "btn btn-secondary btn-sm text-danger-ink hover:border-danger-ink",
                    disabled: edits.pending(),
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
        li { class: "flex items-center justify-between py-3",
            div { class: "min-w-0",
                div { class: "font-medium text-fg-strong truncate", {label.as_str()} }
                div { class: "text-xs font-mono text-fg-muted truncate", {mac} }
            }
            button {
                class: "btn btn-secondary btn-sm",
                disabled: edits.pending(),
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
