//! The Accessories page: the accessories the hub knows, and the ones in range
//! that can be added.

mod edits;
mod hub_sync;
mod radios;
mod status_chip;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::NonZeroU64;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::component::searchable_list::SearchableListItem;
use gpui::component::select::{Select, SelectEvent, SelectState};
use gpui::component::{IndexPath, Sizable as _};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, ElementId, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Rgba, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div, px,
};
use hub_sync::Known;
use odyssey_hub_common::accessory::{
    AccessoryConnectionState, AccessoryInfo, AccessoryInfoMap, AccessoryMap, AccessoryRadio,
    AccessoryRadioInfo, AccessoryType, DiscoveredAccessory,
};
use status_chip::status_chip;

use crate::components::ui::{ButtonKind, button, card, card_title, page};
use crate::hub::{Hub, HubHandle};
use crate::theme::{Palette, palette};
use crate::views::calibrate::automatic::screens::{SCREEN_COUNT, screen_label};
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

fn slot_color_dot(p: &Palette, slot: usize) -> Rgba {
    match slot {
        0 => p.danger,
        1 => p.info,
        2 => p.warning,
        3 => p.success,
        4 => p.accent,
        _ => p.fg_faint,
    }
}

/// Parses a selected UUID. `None` stands for unassigned or invalid.
fn parse_assignment(selected: &str) -> Option<NonZeroU64> {
    let uuid = selected.parse::<u64>().ok()?;
    NonZeroU64::new(uuid)
}

fn parse_screen(selected: &str) -> Option<u8> {
    let id = selected.parse::<u8>().ok()?;
    (usize::from(id) < SCREEN_COUNT).then_some(id)
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
    assignment.map_or_default(|uuid| uuid.to_string())
}

/// An accessory's assignment dropdown, and what it was last set to show.
struct AssignSelect {
    state: Entity<SelectState<Vec<Choice>>>,
    /// `None` forces the next render to set the choices and selection again.
    shown: Option<(Vec<Choice>, String)>,
    _confirm: Subscription,
}

struct NearbyAccessory<'a> {
    accessory: &'a DiscoveredAccessory,
    radios: Vec<&'a AccessoryRadioInfo>,
}

fn nearby_accessories<'a>(
    radios: &'a [AccessoryRadioInfo],
    configured: &AccessoryInfoMap,
) -> Vec<NearbyAccessory<'a>> {
    let mut nearby = BTreeMap::<_, NearbyAccessory<'a>>::new();
    for radio in radios {
        if !radio.available || !radio.accessory_supported {
            continue;
        }
        for accessory in &radio.nearby {
            if configured.contains_key(&accessory.address) {
                continue;
            }
            let entry = nearby
                .entry(accessory.address)
                .or_insert_with(|| NearbyAccessory {
                    accessory,
                    radios: Vec::new(),
                });
            if entry.accessory.name.is_empty() && !accessory.name.is_empty() {
                entry.accessory = accessory;
            }
            if !entry.radios.iter().any(|seen| seen.radio == radio.radio) {
                entry.radios.push(radio);
            }
        }
    }
    nearby.into_values().collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PairingWindow {
    Unknown,
    Closed,
    Open,
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(u64::MAX, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn pairing_window(expires_at_ms: Option<u64>, now_ms: u64) -> PairingWindow {
    match expires_at_ms {
        None => PairingWindow::Unknown,
        Some(deadline) if deadline > now_ms => PairingWindow::Open,
        Some(_) => PairingWindow::Closed,
    }
}

fn pairing_expiry(
    radios: &[AccessoryRadioInfo],
    route: &AccessoryRadio,
    id: [u8; 6],
) -> Option<u64> {
    radios::find_radio(radios, route)?
        .nearby
        .iter()
        .find(|accessory| accessory.address == id)?
        .pairing_window_expires_at_ms
}

fn pairing_hint(expires_at_ms: Option<u64>, p: &Palette) -> gpui::Div {
    let now = unix_time_ms();
    let (text, color) = match pairing_window(expires_at_ms, now) {
        PairingWindow::Open => (
            format!(
                "Pairing window open · {}s remaining",
                expires_at_ms
                    .unwrap_or(now)
                    .saturating_sub(now)
                    .div_ceil(1000)
            ),
            p.success,
        ),
        PairingWindow::Closed => (
            "Locked · Hold board’s power button for 3 seconds to open pairing.".into(),
            p.fg_muted,
        ),
        PairingWindow::Unknown => (
            "Pairing status unavailable · Update marker board and selected radio firmware.".into(),
            p.warning_ink,
        ),
    };
    div().text_sm().text_color(color).child(text)
}

pub(crate) struct AccessoriesView {
    hub: Entity<Hub>,
    known: Known,
    infos: AccessoryInfoMap,
    radio_selects: HashMap<[u8; 6], AssignSelect>,
    radio_drafts: HashMap<[u8; 6], AccessoryRadio>,
    expanded_radios: HashSet<AccessoryRadio>,
    edit_error: Option<String>,
    edit_pending: bool,
    enrolling: Option<[u8; 6]>,
    resetting: Option<[u8; 6]>,
    marker_ota_busy: Option<[u8; 6]>,
    marker_ota_result: Option<([u8; 6], Result<String, String>)>,
    selects: HashMap<[u8; 6], AssignSelect>,
    _hub_sync: Task<()>,
    _pairing_clock: Task<()>,
}

impl AccessoriesView {
    pub(crate) fn new(_window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let hub = cx.global::<HubHandle>().0.clone();
        cx.observe(&hub, |_, _, cx| cx.notify()).detach();
        let pairing_clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let now = unix_time_ms();
                if this
                    .update(cx, |this, cx| {
                        let ticking = this
                            .hub
                            .read(cx)
                            .accessory_radios
                            .iter()
                            .flat_map(|radio| &radio.nearby)
                            .any(|accessory| {
                                accessory
                                    .pairing_window_expires_at_ms
                                    .is_some_and(|deadline| {
                                        deadline != 0 && deadline.saturating_add(1000) > now
                                    })
                            });
                        if ticking {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            hub,
            known: Known::new(),
            infos: AccessoryInfoMap::new(),
            radio_selects: HashMap::new(),
            radio_drafts: HashMap::new(),
            expanded_radios: HashSet::new(),
            edit_error: None,
            edit_pending: false,
            enrolling: None,
            resetting: None,
            marker_ota_busy: None,
            marker_ota_result: None,
            selects: HashMap::new(),
            _hub_sync: hub_sync::follow(cx),
            _pairing_clock: pairing_clock,
        }
    }

    fn publish(&mut self, map: AccessoryMap, cx: &mut Context<'_, Self>) {
        self.infos = map
            .iter()
            .map(|(id, (info, _))| (*id, info.clone()))
            .collect();
        self.known = map;
        // A fresh canonical map means the failure the message described is over.
        self.edit_error = None;
        cx.notify();
    }

    fn is_connected(&self, id: [u8; 6]) -> bool {
        self.known.get(&id).is_some_and(|(_, connected)| *connected)
    }

    /// After a rejected edit, so each dropdown goes back to what the hub has.
    fn forget_shown_assignments(&mut self) {
        for select in self.selects.values_mut() {
            select.shown = None;
        }
        for select in self.radio_selects.values_mut() {
            select.shown = None;
        }
    }

    /// Keeps one dropdown per accessory: screens for boards, trackers for impacts.
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
            let marker = info.ty == AccessoryType::MarkerControlBoard;
            let mut choices = if marker {
                std::iter::once(Choice {
                    value: String::new(),
                    label: "Unassigned".into(),
                })
                .chain((0..SCREEN_COUNT as u8).map(|id| Choice {
                    value: id.to_string(),
                    label: screen_label(id).into(),
                }))
                .collect::<Vec<_>>()
            } else {
                choices.clone()
            };
            let wanted = if marker {
                info.screen_id.map_or_default(|id| id.to_string())
            } else {
                assignment_value(info.assignment)
            };
            if !marker
                && let Some(uuid) = info.assignment
                && !choices.iter().any(|choice| choice.value == wanted)
            {
                choices.push(Choice {
                    value: wanted.clone(),
                    label: format!("0x{:x} (offline)", uuid.get()).into(),
                });
            }
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
                        if marker {
                            let screen = parse_screen(value);
                            if !value.is_empty() && screen.is_none() {
                                return;
                            }
                            if this
                                .infos
                                .get(&id)
                                .is_some_and(|info| info.screen_id != screen)
                            {
                                this.assign_screen(id, screen, cx);
                            }
                            return;
                        }
                        let assignment = parse_assignment(value);
                        if !value.is_empty() && assignment.is_none() {
                            return;
                        }
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
        device_choices: &[DeviceChoice],
        radios: &[AccessoryRadioInfo],
        p: &Palette,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let info = &self.infos[&id];
        let mac = format_address(id);
        let connected = self.is_connected(id);
        let radio = radios::find_radio(radios, &info.radio);
        let connection = radio.and_then(|radio| {
            radio
                .connections
                .iter()
                .find(|connection| connection.address == id)
        });
        let fallback = if radio.is_none_or(|radio| !radio.available) {
            AccessoryConnectionState::RadioUnavailable
        } else if connected {
            AccessoryConnectionState::Connected
        } else if info.ty == AccessoryType::MarkerControlBoard {
            AccessoryConnectionState::Idle
        } else {
            AccessoryConnectionState::Connecting
        };
        let state = connection.map_or(&fallback, |connection| &connection.state);
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

        let mut badges =
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_chip(row_id("status"), state, p));
        if info.ty != AccessoryType::MarkerControlBoard {
            badges = badges.child(
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
        }
        let mut controls = div().w_full().flex().flex_col().gap_2().child(
            div().text_xs().text_color(p.fg_muted).child(
                if info.ty == AccessoryType::MarkerControlBoard {
                    "Screen"
                } else {
                    "Tracker"
                },
            ),
        );
        if let Some(select) = self.selects.get(&id) {
            controls = controls.child(
                Select::new(&select.state)
                    .small()
                    .w(px(240.0))
                    .menu_width(px(320.0))
                    .disabled(self.edit_pending)
                    .accessibility_label(format!(
                        "{} for {}",
                        if info.ty == AccessoryType::MarkerControlBoard {
                            "Screen"
                        } else {
                            "Tracker"
                        },
                        info.name,
                    )),
            );
        }
        let marker = info.ty == AccessoryType::MarkerControlBoard;
        let mut actions = div().flex().items_center().gap_2().child(
            button(row_id("remove"), "Remove", ButtonKind::Secondary, p)
                .small()
                .disabled(self.edit_pending)
                .on_click(cx.listener(move |this, _, _, cx| this.remove(id, cx))),
        );
        if marker {
            let ota_enabled =
                radio.is_some_and(|radio| radio.available && radio.accessory_supported);
            actions = actions.child(
                button(
                    row_id("marker-ota"),
                    if self.marker_ota_busy == Some(id) {
                        "Updating…"
                    } else {
                        "Update over Bluetooth"
                    },
                    ButtonKind::Secondary,
                    p,
                )
                .small()
                .disabled(!ota_enabled || self.marker_ota_busy.is_some())
                .on_click(cx.listener(move |this, _, _, cx| this.update_marker_ota(id, cx))),
            );
            if let Some((result_id, result)) = &self.marker_ota_result
                && *result_id == id
            {
                actions = actions.child(
                    div()
                        .text_sm()
                        .text_color(if result.is_ok() { p.success_ink } else { p.danger_ink })
                        .child(match result {
                            Ok(message) => message.clone(),
                            Err(error) => format!(
                                "Bluetooth update failed: {error}. Reconnect this same marker by USB, then use its firmware update row."
                            ),
                        }),
                );
            }
            actions = actions.child(
                button(
                    row_id("reset-auto"),
                    if self.resetting == Some(id) {
                        "Resetting…"
                    } else {
                        "Reset to Auto"
                    },
                    ButtonKind::Secondary,
                    p,
                )
                .small()
                .disabled(
                    self.edit_pending
                        || !radio.is_some_and(|radio| radio.available && radio.accessory_supported),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.reset_marker_mode(id, cx))),
            );
        }
        controls = controls.child(actions);
        if marker {
            controls = controls.child(
                div()
                    .text_xs()
                    .text_color(p.fg_muted)
                    .child("Use if markers stay lit while Idle (button was clicked)."),
            );
        }

        div()
            .flex()
            .flex_col()
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
                    )
                    .when(info.ty == AccessoryType::MarkerControlBoard, |row| {
                        row.child(
                            div()
                                .text_sm()
                                .text_color(p.fg_muted)
                                .child("Automatic: on while its screen is zeroed or streamed; off when idle or unassigned."),
                        )
                    }),
            )
            .child(badges)
            .when_some(connection.and_then(|connection| connection.error.as_ref()), |row, error| {
                row.child(div().text_sm().text_color(p.danger).child(radios::radio_error_summary(error).into_owned()))
            })
            .child(controls)
            .child(self.radio_controls(id, info, radios, p, cx))
            .into_any_element()
    }

    fn nearby_row(
        &self,
        index: usize,
        nearby: &NearbyAccessory<'_>,
        p: &Palette,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let accessory = nearby.accessory;
        let id = accessory.address;
        let ty = accessory.ty;
        let mac = format_address(id);
        let name = if accessory.name.is_empty() {
            ty.name().to_string()
        } else {
            accessory.name.clone()
        };
        let route = self
            .radio_drafts
            .get(&id)
            .unwrap_or(&nearby.radios[0].radio)
            .clone();
        let available = nearby.radios.iter().any(|radio| radio.radio == route);
        let expires = nearby
            .radios
            .iter()
            .find(|radio| radio.radio == route)
            .and_then(|radio| radio.nearby.iter().find(|seen| seen.address == id))
            .and_then(|seen| seen.pairing_window_expires_at_ms);
        let can_enroll = ty != AccessoryType::MarkerControlBoard
            || pairing_window(expires, unix_time_ms()) == PairingWindow::Open;
        let mut controls = div().w_full().flex().flex_col().gap_2();
        if let Some(select) = self.radio_selects.get(&id) {
            controls = controls.child(
                Select::new(&select.state)
                    .small()
                    .w(px(240.0))
                    .menu_width(px(320.0))
                    .disabled(self.edit_pending)
                    .accessibility_label(format!("Radio for {name}")),
            );
        }
        controls = controls.child(
            div().flex().child(
                button(
                    (ElementId::from(mac.clone()), "add"),
                    if self.enrolling == Some(id) {
                        "Enrolling…"
                    } else {
                        "Pair & add"
                    },
                    ButtonKind::Secondary,
                    p,
                )
                .small()
                .disabled(self.edit_pending || !available || !can_enroll)
                .on_click(cx.listener({
                    let name = name.clone();
                    move |this, _, _, cx| this.add(id, name.clone(), ty, route.clone(), cx)
                })),
            ),
        );
        div()
            .flex()
            .flex_col()
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
                            .child(name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(MONO_FONT)
                            .text_color(p.fg_muted)
                            .truncate()
                            .child(mac),
                    )
                    .child(div().text_xs().text_color(p.fg_muted).child(format!(
                        "Seen by {} {}",
                        nearby.radios.len(),
                        if nearby.radios.len() == 1 {
                            "radio"
                        } else {
                            "radios"
                        },
                    ))),
            )
            .when(ty == AccessoryType::MarkerControlBoard, |row| {
                row.child(pairing_hint(expires, p))
            })
            .child(controls)
            .into_any_element()
    }
}

impl Render for AccessoriesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let p = &palette(cx);

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
        let radios = self.hub.read(cx).accessory_radios.clone();
        let nearby = nearby_accessories(&radios, &self.infos);
        self.sync_radio_selects(&radios, &nearby, window, cx);

        let empty = |text: &str| {
            div()
                .mt_1()
                .text_sm()
                .text_color(p.fg_muted)
                .child(text.to_string())
        };
        let mut groups = vec![
            self.radios_panel(&radios, p, cx),
            card(p)
                .child(card_title(p, "Pairing and radio ownership"))
                .child(empty("Nearby shows detected accessories, including locked boards. For marker boards, hold the power button for 3 seconds and wait for Pairing window open before choosing Pair & add or Pair / enroll selected radio. Moving to another radio changes the board’s approved bond owner and needs a new physical pairing window. Keep the accessory nearby until enrollment finishes."))
                .child(empty("Physical pairing uses Bluetooth Just Works; it does not provide authenticated protection against an active nearby attacker."))
                .into_any_element(),
        ];
        if let Some(error) = &self.hub.read(cx).accessory_radio_error {
            groups.push(
                card(p)
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.warning_ink)
                            .child(error.clone()),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = &self.edit_error {
            groups.push(
                card(p)
                    .child(div().text_sm().text_color(p.danger).child(error.clone()))
                    .into_any_element(),
            );
        }
        for (first_party, title) in [(true, "Odyssey"), (false, "Third-party")] {
            let supported = AccessoryType::ALL
                .iter()
                .filter(|ty| ty.is_first_party() == first_party)
                .map(|ty| ty.name())
                .collect::<Vec<_>>()
                .join(", ");
            let mut group = card(p)
                .child(card_title(p, title))
                .child(empty(&format!("Supported: {supported}")))
                .child(
                    div()
                        .mt_4()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.fg_strong)
                        .child("Configured"),
                );
            let mut added_ids: Vec<_> = self
                .infos
                .iter()
                .filter(|(_, info)| info.ty.is_first_party() == first_party)
                .map(|(id, _)| *id)
                .collect();
            added_ids.sort_by_key(|id| (!self.is_connected(*id), *id));
            if added_ids.is_empty() {
                group = group.child(empty("None configured. Add an accessory from Nearby."));
            } else {
                let rows: Vec<_> = added_ids
                    .iter()
                    .enumerate()
                    .map(|(index, id)| self.added_row(index, *id, &device_choices, &radios, p, cx))
                    .collect();
                group = group.child(div().flex().flex_col().children(rows));
            }
            group = group.child(
                div()
                    .mt_4()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.fg_strong)
                    .child("Nearby"),
            );
            let addable: Vec<_> = nearby
                .iter()
                .filter(|entry| entry.accessory.ty.is_first_party() == first_party)
                .collect();
            if addable.is_empty() {
                group = group.child(empty(
                    "No new accessories reported by the hub. Switch one on and keep it nearby; each available radio refreshes discovery automatically.",
                ));
            } else {
                let rows: Vec<_> = addable
                    .into_iter()
                    .enumerate()
                    .map(|(index, entry)| self.nearby_row(index, entry, p, cx))
                    .collect();
                group = group.child(div().flex().flex_col().children(rows));
            }
            groups.push(group.into_any_element());
        }

        page(
            p,
            "Accessories",
            Some("Configure Odyssey marker control boards and supported third-party Bluetooth accessories.".into()),
            groups,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discovered_radio(id: &str, name: &str) -> AccessoryRadioInfo {
        AccessoryRadioInfo {
            radio: AccessoryRadio::Dongle { id: id.into() },
            name: id.into(),
            is_default: false,
            available: true,
            accessory_supported: true,
            connected_accessories: 0,
            connecting_accessories: 0,
            connected_trackers: 0,
            capacity: Some(6),
            accessory_capacity: Some(3),
            connections: Vec::new(),
            nearby: vec![DiscoveredAccessory {
                address: [1; 6],
                name: name.into(),
                ty: AccessoryType::MarkerControlBoard,
                pairing_window_expires_at_ms: Some(0),
            }],
            error: None,
        }
    }

    #[test]
    fn nearby_groups_one_board_and_preserves_eligible_radio_choices() {
        let first = discovered_radio("first", "");
        let second = discovered_radio("second", "Odyssey Markers");
        let mut unavailable = discovered_radio("unavailable", "Wrong name");
        unavailable.available = false;
        let mut unsupported = discovered_radio("unsupported", "Wrong name");
        unsupported.accessory_supported = false;
        let radios = vec![first, second, unavailable, unsupported];
        let groups = nearby_accessories(&radios, &AccessoryInfoMap::new());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].accessory.address, [1; 6]);
        assert_eq!(groups[0].accessory.name, "Odyssey Markers");
        assert_eq!(
            groups[0]
                .radios
                .iter()
                .map(|radio| &radio.radio)
                .collect::<Vec<_>>(),
            vec![&radios[0].radio, &radios[1].radio],
        );
        let configured = AccessoryInfoMap::from([(
            [1; 6],
            AccessoryInfo::with_defaults("Saved markers", AccessoryType::MarkerControlBoard),
        )]);
        assert!(nearby_accessories(&radios, &configured).is_empty());
    }

    #[test]
    fn pairing_permission_uses_selected_radio_and_expires_at_deadline() {
        let locked = discovered_radio("locked", "Odyssey Markers");
        let mut open = discovered_radio("open", "Odyssey Markers");
        open.nearby[0].pairing_window_expires_at_ms = Some(30_000);
        let radios = [locked, open];
        assert_eq!(pairing_window(None, 0), PairingWindow::Unknown);
        assert_eq!(pairing_window(Some(0), 0), PairingWindow::Closed);
        assert_eq!(
            pairing_window(pairing_expiry(&radios, &radios[0].radio, [1; 6]), 1),
            PairingWindow::Closed,
        );
        let expires = pairing_expiry(&radios, &radios[1].radio, [1; 6]);
        assert_eq!(pairing_window(expires, 29_999), PairingWindow::Open);
        assert_eq!(pairing_window(expires, 30_000), PairingWindow::Closed);
        assert_eq!(pairing_window(expires, u64::MAX), PairingWindow::Closed);
    }

    #[test]
    fn the_dropdown_value_maps_to_an_assignment() {
        assert_eq!(parse_assignment(""), None);
        assert_eq!(parse_assignment("42"), NonZeroU64::new(42));
        // Zero is not a device and text is not a choice: the edit is ignored.
        assert_eq!(parse_assignment("0"), None);
        assert_eq!(parse_assignment("abc"), None);
    }

    #[test]
    fn screen_assignment_accepts_all_six_screens_without_tracker_rules() {
        assert_eq!(parse_screen(""), None);
        for id in 0..6 {
            assert_eq!(parse_screen(&id.to_string()), Some(id));
        }
        assert_eq!(parse_screen("6"), None);
        assert_eq!(parse_screen("255"), None);
        assert_eq!(parse_screen("abc"), None);
    }
}
