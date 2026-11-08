//! Radio ownership is independent of screen/tracker assignment.

use super::*;

pub(super) fn radio_value(radio: &AccessoryRadio) -> String {
    match radio {
        AccessoryRadio::System => "system".into(),
        AccessoryRadio::Native { id } => format!("native:{id}"),
        AccessoryRadio::Dongle { id } => format!("dongle:{id}"),
    }
}

fn parse_radio(value: &str) -> Option<AccessoryRadio> {
    if value == "system" {
        return Some(AccessoryRadio::System);
    }
    let (kind, id) = value.split_once(':')?;
    if id.is_empty() {
        return None;
    }
    match kind {
        "native" => Some(AccessoryRadio::Native { id: id.into() }),
        "dongle" => Some(AccessoryRadio::Dongle { id: id.into() }),
        _ => None,
    }
}

pub(super) fn find_radio<'a>(
    radios: &'a [AccessoryRadioInfo],
    selected: &AccessoryRadio,
) -> Option<&'a AccessoryRadioInfo> {
    radios.iter().find(|info| {
        &info.radio == selected || (*selected == AccessoryRadio::System && info.is_default)
    })
}

fn radio_label(radios: &[AccessoryRadioInfo], selected: &AccessoryRadio) -> String {
    match find_radio(radios, selected) {
        Some(info) => format!(
            "{}{}",
            info.name,
            if !info.available {
                " (unavailable)"
            } else if !info.accessory_supported {
                " (firmware update required)"
            } else {
                ""
            }
        ),
        None => format!("{} (unavailable)", radio_value(selected)),
    }
}

impl AccessoriesView {
    pub(super) fn sync_radio_selects(
        &mut self,
        radios: &[AccessoryRadioInfo],
        nearby: &[NearbyAccessory<'_>],
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let visible = |id: &[u8; 6]| {
            self.infos.contains_key(id) || nearby.iter().any(|entry| entry.accessory.address == *id)
        };
        self.radio_selects.retain(|id, _| visible(id));
        self.radio_drafts.retain(|id, _| visible(id));
        self.expanded_radios
            .retain(|radio| radios.iter().any(|info| &info.radio == radio));
        for entry in nearby {
            self.radio_drafts
                .entry(entry.accessory.address)
                .or_insert_with(|| {
                    entry
                        .radios
                        .iter()
                        .find(|radio| radio.is_default)
                        .unwrap_or(&entry.radios[0])
                        .radio
                        .clone()
                });
        }
        for id in self
            .infos
            .keys()
            .copied()
            .chain(nearby.iter().map(|entry| entry.accessory.address))
        {
            let discovered = nearby.iter().find(|entry| entry.accessory.address == id);
            let Some(wanted) = self
                .radio_drafts
                .get(&id)
                .or_else(|| self.infos.get(&id).map(|info| &info.radio))
            else {
                continue;
            };
            // `System` means "whichever radio is default", so it must select that radio's
            // own entry instead of adding a second choice with the same label.
            let wanted_value = find_radio(radios, wanted)
                .map_or_else(|| radio_value(wanted), |info| radio_value(&info.radio));
            let mut choices: Vec<_> = radios
                .iter()
                .filter(|radio| radio.available && radio.accessory_supported)
                .filter(|radio| {
                    discovered.is_none_or(|entry| {
                        entry.radios.iter().any(|seen| seen.radio == radio.radio)
                    })
                })
                .map(|radio| Choice {
                    value: radio_value(&radio.radio),
                    label: radio_label(radios, &radio.radio).into(),
                })
                .collect();
            if !choices.iter().any(|choice| choice.value == wanted_value) {
                choices.push(Choice {
                    value: wanted_value.clone(),
                    label: if discovered.is_some() {
                        format!(
                            "{} (not discovering this accessory)",
                            radio_label(radios, wanted)
                        )
                        .into()
                    } else {
                        radio_label(radios, wanted).into()
                    },
                });
            }
            let selected = choices
                .iter()
                .position(|choice| choice.value == wanted_value)
                .map(IndexPath::new);
            let select = self.radio_selects.entry(id).or_insert_with(|| {
                let state = cx.new(|cx| SelectState::new(choices.clone(), selected, window, cx));
                let confirm = cx.subscribe(
                    &state,
                    move |this, _, event: &SelectEvent<Vec<Choice>>, cx| {
                        let SelectEvent::Confirm(Some(value)) = event else {
                            return;
                        };
                        let Some(radio) = parse_radio(value) else {
                            return;
                        };
                        if this.edit_pending {
                            // The select kept the user's pick; force a resync to the saved radio.
                            this.forget_shown_assignments();
                            cx.notify();
                            return;
                        }
                        this.radio_drafts.insert(id, radio);
                        cx.notify();
                    },
                );
                AssignSelect {
                    state,
                    shown: Some((choices.clone(), wanted_value.clone())),
                    _confirm: confirm,
                }
            });
            let current = (choices, wanted_value);
            if select.shown.as_ref() != Some(&current) {
                select.state.update(cx, |state, cx| {
                    state.set_items(current.0.clone(), window, cx);
                    state.set_selected_index(selected, window, cx);
                });
                select.shown = Some(current);
            }
        }
    }

    pub(super) fn radio_controls(
        &self,
        id: [u8; 6],
        info: &AccessoryInfo,
        radios: &[AccessoryRadioInfo],
        p: Palette,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let draft = self.radio_drafts.get(&id).unwrap_or(&info.radio);
        let available = find_radio(radios, draft)
            .is_some_and(|radio| radio.available && radio.accessory_supported);
        let expires = pairing_expiry(radios, draft, &id);
        let can_enroll = info.ty != AccessoryType::MarkerControlBoard
            || pairing_window(expires, unix_time_ms()) == PairingWindow::Open;
        let mut controls = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_xs().text_color(p.fg_muted).child("Radio"));
        if let Some(select) = self.radio_selects.get(&id) {
            controls = controls.child(
                Select::new(&select.state)
                    .small()
                    .w(px(240.0))
                    .menu_width(px(320.0))
                    .disabled(self.edit_pending)
                    .accessibility_label(format!("Radio for {}", info.name)),
            );
        }
        if info.ty == AccessoryType::MarkerControlBoard {
            controls = controls.child(pairing_hint(expires, p));
        }
        let mut next = info.clone();
        next.radio = draft.clone();
        controls
            .child(
                div().flex().child(
                    button(
                        (ElementId::from(format_address(&id)), "enroll-radio"),
                        if self.enrolling == Some(id) {
                            "Enrolling…"
                        } else {
                            "Pair / enroll selected radio"
                        },
                        ButtonKind::Secondary,
                        p,
                    )
                    .small()
                    .disabled(self.edit_pending || !available || !can_enroll)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.enroll(id, next.clone(), cx);
                    })),
                ),
            )
            .child(div().text_xs().text_color(p.fg_muted).child(format!(
                "Saved radio: {}. Selection takes effect only after enrollment succeeds.",
                radio_label(radios, &info.radio),
            )))
            .into_any_element()
    }

    pub(super) fn radios_panel(
        &self,
        radios: &[AccessoryRadioInfo],
        p: Palette,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let mut panel = card(p).child(card_title(p, "Radios"));
        if radios.is_empty() {
            panel = panel.child(div().mt_3().text_sm().text_color(p.fg_muted).child(
                "No radio snapshot available. Connect the hub and enable Bluetooth or attach an Odyssey dongle.",
            ));
        }
        let mut cards = div().mt_3().flex().flex_wrap().items_start().gap_3();
        for radio in radios {
            let is_dongle = matches!(radio.radio, AccessoryRadio::Dongle { .. });
            let expanded = self.expanded_radios.contains(&radio.radio);
            let stable_id = radio_value(&radio.radio);
            let selected_radio = radio.radio.clone();
            let capacity = if is_dongle {
                radio
                    .capacity
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".into())
            } else {
                "unknown".into()
            };
            let mut row = card(p)
                .w(px(280.0))
                .min_w_0()
                .max_w_full()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.fg_strong)
                        .child(radio.name.clone()),
                )
                .child(div().mt_1().text_xs().text_color(p.fg_muted).child(format!(
                    "{} · {}{}",
                    if radio.available {
                        "Available"
                    } else {
                        "Unavailable"
                    },
                    if radio.accessory_supported {
                        "Accessories supported"
                    } else {
                        "Accessories unsupported"
                    },
                    if radio.is_default { " · default" } else { "" },
                )))
                .child(div().mt_2().text_sm().text_color(p.fg_muted).child(format!(
                    "Odyssey live links: {} / {} · {} pending",
                    radio.connected_trackers + radio.connected_accessories,
                    capacity,
                    radio.connecting_accessories,
                )));
            if !radio.available {
                row = row.child(div().mt_1().text_sm().text_color(p.warning_ink).child(
                    "Fixed assignments stay on this radio while unavailable; no automatic migration.",
                ));
            }
            if radio.available && is_dongle && !radio.accessory_supported {
                row = row.child(div().mt_1().text_sm().text_color(p.warning_ink).child(
                    "Update this Odyssey dongle’s firmware to enable accessory routing. Tracker operation is unchanged.",
                ));
            }
            if let Some(error) = &radio.error {
                row = row.child(
                    div()
                        .mt_1()
                        .text_sm()
                        .text_color(p.warning_ink)
                        .child(error.clone()),
                );
            }
            row = row.child(
                div().mt_2().flex().child(
                    button(
                        (ElementId::from(stable_id.clone()), "radio-details"),
                        if expanded { "Hide details" } else { "Details" },
                        ButtonKind::Secondary,
                        p,
                    )
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded_radios.remove(&selected_radio) {
                            this.expanded_radios.insert(selected_radio.clone());
                        }
                        cx.notify();
                    })),
                ),
            );
            if expanded {
                row = row.child(
                    div()
                        .mt_3()
                        .pt_3()
                        .border_t_1()
                        .border_color(p.line)
                        .child(
                            div()
                                .id((ElementId::from(stable_id.clone()), "radio-stable-id"))
                                .overflow_x_scroll()
                                .text_xs()
                                .font_family(MONO_FONT)
                                .text_color(p.fg_muted)
                                .child(stable_id),
                        )
                        .child(div().mt_2().text_sm().text_color(p.fg_muted).child(format!(
                            "{} trackers + {} accessories; {} pending accessory attempts.",
                            radio.connected_trackers,
                            radio.connected_accessories,
                            radio.connecting_accessories,
                        )))
                        .child(div().mt_2().text_sm().text_color(p.fg_muted).child(
                            if is_dongle {
                                format!(
                                    "Reported total link budget: {}; accessory budget: {}.",
                                    capacity,
                                    radio.accessory_capacity.map(|n| n.to_string()).unwrap_or_else(|| "unknown".into()),
                                )
                            } else {
                                "Native global usage and capacity are unknown; limits depend on the controller and other applications.".into()
                            },
                        ))
                        .child(div().mt_2().text_sm().text_color(p.fg_muted).child(
                            "Counts show Odyssey-owned physical links only, not all Bluetooth use on this computer. Pending attempts are separate from live links. Fixed assignments stay on their radio while unavailable; no automatic migration.",
                        ))
                        .child(div().mt_2().text_sm().text_color(p.fg_muted).child(
                            if cfg!(target_os = "linux") {
                                "Need more Bluetooth capacity? Assign accessories to an additional native Bluetooth adapter or Odyssey USB dongle. Native limits depend on the controller and other applications."
                            } else {
                                "Need more Bluetooth capacity? Assign accessories to an additional Odyssey USB dongle. This OS exposes one system Bluetooth radio; native limits depend on the controller and other applications."
                            },
                        )),
                );
            }
            cards = cards.child(row);
        }
        panel.child(cards).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radio_selection_preserves_kind_and_complete_stable_id() {
        for radio in [
            AccessoryRadio::System,
            AccessoryRadio::Native {
                id: "AA:BB:CC:DD:EE:FF".into(),
            },
            AccessoryRadio::Dongle {
                id: "00123456789abcdef".into(),
            },
        ] {
            assert_eq!(parse_radio(&radio_value(&radio)), Some(radio));
        }
        for invalid in ["", "native:", "dongle:", "system:other", "other:id"] {
            assert_eq!(parse_radio(invalid), None);
        }
    }

    #[test]
    fn fixed_radio_never_resolves_to_another_default_radio() {
        let selected = AccessoryRadio::Dongle {
            id: "unplugged".into(),
        };
        let default = AccessoryRadioInfo {
            radio: AccessoryRadio::Native {
                id: "native-address".into(),
            },
            name: "Native radio".into(),
            is_default: true,
            available: true,
            accessory_supported: true,
            connected_accessories: 9,
            connecting_accessories: 1,
            connected_trackers: 0,
            capacity: None,
            accessory_capacity: None,
            connections: Vec::new(),
            nearby: Vec::new(),
            error: None,
        };
        let mut radios = vec![default];
        assert!(find_radio(&radios, &selected).is_none());
        assert_eq!(
            find_radio(&radios, &AccessoryRadio::System).map(|radio| &radio.radio),
            Some(&radios[0].radio),
        );
        let mut unavailable = radios[0].clone();
        unavailable.radio = selected.clone();
        unavailable.is_default = false;
        unavailable.available = false;
        radios.push(unavailable);
        let resolved = find_radio(&radios, &selected).unwrap();
        assert_eq!(resolved.radio, selected);
        assert!(!resolved.available);
    }
}
