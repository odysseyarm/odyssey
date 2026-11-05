//! The Accessories page: the accessories the hub knows, and the ones in range
//! that can be added.

mod ble_scan;
mod edits;
mod hub_sync;
mod status_chip;

use std::collections::HashMap;
use std::num::NonZeroU64;

use ble_scan::BleScan;
use gpui::component::searchable_list::SearchableListItem;
use gpui::component::select::{Select, SelectEvent, SelectState};
use gpui::component::{IndexPath, Sizable as _};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, ElementId, Entity, FontWeight, IntoElement,
    ParentElement as _, Render, Rgba, SharedString, Styled as _, Subscription, Task, Window, div,
    px,
};
use hub_sync::Known;
use odyssey_hub_common::accessory::{AccessoryInfo, AccessoryInfoMap, AccessoryMap, AccessoryType};
use status_chip::status_chip;

use crate::components::ui::{ButtonKind, button, card, card_title, page};
use crate::hub::{Hub, HubHandle};
use crate::theme::{Palette, palette};
use crate::views::device_label;
use crate::views::device_label::format_address;

/// The slot color shown for an accessory whose device is unknown.
const UNASSIGNED_SLOT: usize = 5;

/// The font of ids and addresses.
const MONO_FONT: &str = if cfg!(windows) {
    "Consolas"
} else {
    "monospace"
};

fn slot_color_dot(p: Palette, slot: usize) -> Rgba {
    match slot {
        0 => p.danger,
        1 => p.info,
        2 => p.warning,
        3 => p.success,
        4 => p.accent,
        _ => p.fg_faint,
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

/// A connected device an accessory can be assigned to: the uuid the hub
/// matches on, how to label it, and its slot.
type DeviceChoice = (NonZeroU64, String, usize);

/// One entry of an assignment dropdown. The value is what
/// [`parse_assignment`] reads: empty for "Unassigned", else the uuid.
#[derive(Clone, PartialEq)]
struct Choice {
    value: String,
    label: SharedString,
}

impl SearchableListItem for Choice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &String {
        &self.value
    }
}

fn assignment_value(assignment: Option<NonZeroU64>) -> String {
    assignment.map(|uuid| uuid.to_string()).unwrap_or_default()
}

/// An accessory's assignment dropdown, and what it was last set to show.
struct AssignSelect {
    state: Entity<SelectState<Vec<Choice>>>,
    /// `None` forces the next render to set the choices and selection again.
    shown: Option<(Vec<Choice>, String)>,
    _confirm: Subscription,
}

pub(crate) struct AccessoriesView {
    hub: Entity<Hub>,
    known: Known,
    infos: AccessoryInfoMap,
    scan: BleScan,
    edit_pending: bool,
    selects: HashMap<[u8; 6], AssignSelect>,
    _hub_sync: Task<()>,
    _ble_scan: Task<()>,
}

impl AccessoriesView {
    pub(crate) fn new(_window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let hub = cx.global::<HubHandle>().0.clone();
        cx.observe(&hub, |_, _, cx| cx.notify()).detach();
        Self {
            hub,
            known: Known::new(),
            infos: AccessoryInfoMap::new(),
            scan: BleScan::Starting,
            edit_pending: false,
            selects: HashMap::new(),
            _hub_sync: hub_sync::follow(cx),
            _ble_scan: ble_scan::follow(cx),
        }
    }

    fn publish(&mut self, map: AccessoryMap, cx: &mut Context<'_, Self>) {
        self.infos = map
            .iter()
            .map(|(id, (info, _))| (*id, info.clone()))
            .collect();
        self.known = map;
        cx.notify();
    }

    fn is_connected(&self, id: &[u8; 6]) -> bool {
        self.known.get(id).is_some_and(|(_, connected)| *connected)
    }

    /// After a rejected edit, so each dropdown goes back to what the hub has.
    fn forget_shown_assignments(&mut self) {
        for select in self.selects.values_mut() {
            select.shown = None;
        }
    }

    /// Keeps one dropdown per added accessory, showing its assignment among
    /// the current devices.
    fn sync_selects(
        &mut self,
        device_choices: &[DeviceChoice],
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.selects.retain(|id, _| self.infos.contains_key(id));

        let mut choices = vec![Choice {
            value: String::new(),
            label: "Unassigned".into(),
        }];
        choices.extend(device_choices.iter().map(|(uuid, label, slot)| Choice {
            value: uuid.to_string(),
            label: format!("{label} (slot {})", slot + 1).into(),
        }));

        for (id, info) in &self.infos {
            let wanted = assignment_value(info.assignment);
            let selected = choices
                .iter()
                .position(|choice| choice.value == wanted)
                .map(IndexPath::new);
            let select = self.selects.entry(*id).or_insert_with(|| {
                let state = cx.new(|cx| SelectState::new(choices.clone(), selected, window, cx));
                let id = *id;
                let confirm = cx.subscribe(
                    &state,
                    move |this, _, event: &SelectEvent<Vec<Choice>>, cx| {
                        let SelectEvent::Confirm(Some(value)) = event else {
                            return;
                        };
                        let Some(assignment) = parse_assignment(value) else {
                            return;
                        };
                        if this
                            .infos
                            .get(&id)
                            .is_some_and(|info| info.assignment == assignment)
                        {
                            return;
                        }
                        this.assign(id, assignment, cx);
                    },
                );
                AssignSelect {
                    state,
                    shown: Some((choices.clone(), wanted.clone())),
                    _confirm: confirm,
                }
            });
            let current = (choices.clone(), wanted);
            if select.shown.as_ref() != Some(&current) {
                select.state.update(cx, |state, cx| {
                    state.set_items(current.0.clone(), window, cx);
                    state.set_selected_index(selected, window, cx);
                });
                select.shown = Some(current);
            }
        }
    }

    fn added_row(
        &self,
        index: usize,
        id: [u8; 6],
        info: &AccessoryInfo,
        device_choices: &[DeviceChoice],
        p: Palette,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let mac = format_address(&id);
        let connected = self.is_connected(&id);
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
        let dot = slot_color_dot(p, assigned_slot.unwrap_or(UNASSIGNED_SLOT));
        let row_id =
            |part: &'static str| -> ElementId { (ElementId::from(mac.clone()), part).into() };

        let mut controls = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_end()
            .gap_3()
            .child(status_chip(row_id("status"), connected, p))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_full()
                    .px_2()
                    .py_0p5()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .bg(p.surface_alt)
                    .text_color(p.fg_muted)
                    .child(div().size_2().rounded_full().bg(dot))
                    .child(assigned_label),
            );
        if let Some(select) = self.selects.get(&id) {
            controls = controls.child(
                Select::new(&select.state)
                    .small()
                    .w(px(240.0))
                    .menu_width(px(320.0))
                    .disabled(self.edit_pending)
                    .accessibility_label(format!("Device for {}", info.name)),
            );
        }
        controls = controls.child(
            button(row_id("remove"), "Remove", ButtonKind::Secondary, p)
                .small()
                .disabled(self.edit_pending)
                .on_click(cx.listener(move |this, _, _, cx| this.remove(id, cx))),
        );

        div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .py_3()
            .when(index > 0, |row| row.border_t_1().border_color(p.line))
            .child(
                div()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.fg_strong)
                            .truncate()
                            .child(info.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(MONO_FONT)
                            .text_color(p.fg_muted)
                            .truncate()
                            .child(mac.clone()),
                    ),
            )
            .child(controls)
            .into_any_element()
    }

    fn nearby_row(
        &self,
        index: usize,
        id: [u8; 6],
        label: &str,
        ty: AccessoryType,
        p: Palette,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let mac = format_address(&id);
        let name = label.to_string();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .py_3()
            .when(index > 0, |row| row.border_t_1().border_color(p.line))
            .child(
                div()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.fg_strong)
                            .truncate()
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(MONO_FONT)
                            .text_color(p.fg_muted)
                            .truncate()
                            .child(mac.clone()),
                    ),
            )
            .child(
                button(
                    (ElementId::from(mac), "add"),
                    "Add",
                    ButtonKind::Secondary,
                    p,
                )
                .small()
                .disabled(self.edit_pending)
                .on_click(cx.listener(move |this, _, _, cx| this.add(id, name.clone(), ty, cx))),
            )
            .into_any_element()
    }
}

impl Render for AccessoriesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let p = palette(cx);

        let device_choices: Vec<DeviceChoice> = self
            .hub
            .read(cx)
            .devices
            .iter()
            .enumerate()
            .filter_map(|(slot, device)| {
                let mut bytes = [0u8; 8];
                bytes[..6].copy_from_slice(&device.uuid);
                let uuid_u64 = u64::from_le_bytes(bytes);
                Some((NonZeroU64::new(uuid_u64)?, device_label(device), slot))
            })
            .collect();
        self.sync_selects(&device_choices, window, cx);

        // Connected first, then by MAC.
        let mut added_ids: Vec<[u8; 6]> = self.infos.keys().copied().collect();
        added_ids.sort_by_key(|id| (!self.is_connected(id), *id));

        let connected_count = self
            .known
            .values()
            .filter(|(_, connected)| *connected)
            .count();
        let total_count = added_ids.len();

        let empty = |text: &str| {
            div()
                .mt_1()
                .text_sm()
                .text_color(p.fg_muted)
                .child(text.to_string())
        };

        let mut added = card(p).child(
            card_title(p, "Added")
                .flex()
                .items_baseline()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(p.fg_muted)
                        .child(format!("{connected_count} of {total_count} connected")),
                ),
        );
        if added_ids.is_empty() {
            added = added.child(empty("None yet. Add one from the nearby list below."));
        } else {
            let rows: Vec<AnyElement> = added_ids
                .iter()
                .enumerate()
                .map(|(index, id)| {
                    self.added_row(index, *id, &self.infos[id], &device_choices, p, cx)
                })
                .collect();
            added = added.child(div().flex().flex_col().children(rows));
        }

        let mut nearby = card(p).child(card_title(p, "Nearby"));
        nearby = match &self.scan {
            BleScan::Starting => nearby.child(empty("Looking for accessories…")),
            BleScan::Unavailable(reason) => nearby.child(
                div()
                    .mt_1()
                    .text_sm()
                    .text_color(p.warning_ink)
                    .child(format!(
                        "Bluetooth is unavailable, so nearby accessories cannot be found: {reason}."
                    )),
            ),
            BleScan::Found(found) => {
                let mut addable: Vec<(&[u8; 6], &(String, AccessoryType))> = found
                    .iter()
                    .filter(|(id, _)| !self.infos.contains_key(*id))
                    .collect();
                addable.sort_by_key(|(id, _)| **id);
                if addable.is_empty() {
                    nearby.child(empty(
                        "No new accessories in range. Switch one on and keep it near this computer; the list refreshes every few seconds.",
                    ))
                } else {
                    let rows: Vec<AnyElement> = addable
                        .into_iter()
                        .enumerate()
                        .map(|(index, (id, (label, ty)))| {
                            self.nearby_row(index, *id, label, *ty, p, cx)
                        })
                        .collect();
                    nearby.child(div().flex().flex_col().children(rows))
                }
            }
        };

        page(
            p,
            "Accessories",
            Some("Bluetooth accessories such as a DryFireMag or MantisX. Assign one to a device to pair their events.".into()),
            [added.into_any_element(), nearby.into_any_element()],
        )
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
