//! Display and tracker context for calibration, alignment and device tasks.

use gpui::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _, div};

use super::{HomeEvent, HomeView};
use crate::components::ui::{self, AlertKind, ButtonKind, button};
use crate::theme::Palette;
use crate::workspace::Alignment;

impl HomeView {
    pub(super) fn screen_actions(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.selection.read(cx).selected_key();
        let workspace = self.workspace.read(cx);
        let selected_uuid = workspace.selected_device;
        let hub = self.hub.read(cx);
        let device =
            selected_uuid.and_then(|uuid| hub.devices.iter().find(|device| device.uuid == uuid));
        let connected = hub.client.is_some();
        let connection = match device {
            Some(device) if device.events_connected => "Connected · event stream available",
            Some(_) => "Connected · event stream unavailable",
            None if selected_uuid.is_some() => "Selected tracker disconnected",
            None if !connected => "Connecting to Odyssey hub…",
            None if !hub.devices.is_empty() => "Choose a tracker above",
            None => "Connect a tracker to begin alignment",
        };
        let calibration = if workspace.calibration_loading {
            "Checking saved calibration…".to_owned()
        } else if workspace.calibration_error.is_some() {
            "Needs attention".to_owned()
        } else if let Some(id) = selected
            .as_ref()
            .and_then(|key| workspace.calibrations.get(key))
        {
            format!("Saved · Screen {}", u32::from(*id) + 1)
        } else if selected.is_some() && !workspace.saved_calibration_ids.is_empty() {
            "Saved calibration · display association unknown".to_owned()
        } else if selected.is_some() {
            "Not calibrated for this display".to_owned()
        } else {
            "Choose a display".to_owned()
        };
        let alignment = selected_uuid.map_or("Choose a tracker", |uuid| {
            match workspace.alignment(&uuid) {
                Alignment::NotVerified => "Not checked this session",
                Alignment::Applied => "Applied · check aim and save",
                Alignment::Saved => "Saved this session",
            }
        });
        let calibration_error = workspace.calibration_error.clone();
        let devices = hub.devices.clone();
        let mut tracker_choices = div().flex().flex_wrap().gap_2();
        for device in devices {
            let uuid = device.uuid;
            tracker_choices = tracker_choices.child(
                button(
                    (
                        "select-tracker",
                        uuid.iter()
                            .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte)),
                    ),
                    crate::views::device_label(&device),
                    if Some(uuid) == selected_uuid {
                        ButtonKind::Primary
                    } else {
                        ButtonKind::Secondary
                    },
                    p,
                )
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.select_device(uuid, cx));
                })),
            );
        }
        if selected_uuid.is_some() && device.is_none() {
            tracker_choices = tracker_choices.child(
                div()
                    .text_sm()
                    .text_color(p.warning_ink)
                    .child("Reconnect selected tracker, or choose another above."),
            );
        }
        let mut panel = ui::card(p)
            .gap_4()
            .child(ui::card_title(p, "Display"))
            .child(self.picker.clone())
            .child(div().flex().flex_wrap().items_center().justify_between().gap_3()
                .child(div().min_w_0().flex_1().text_sm().child(format!("Screen calibration: {calibration}")))
                .child(button("calibrate-screen", "Calibrate screen", ButtonKind::Secondary, p)
                    .disabled(selected.is_none())
                    .on_click(|_, _, cx| crate::views::calibrate::automatic::open(None, cx))))
            .child(ui::card_title(p, "Tracker").mt_2())
            .child(tracker_choices)
            .child(div().text_sm().text_color(p.fg_muted).child(connection))
            .child(div().text_sm().child(format!("Tracker alignment: {alignment}")))
            .child(div().text_sm().text_color(p.fg_muted).child("Tracking readiness is checked in Zero. A connected tracker is not necessarily tracking the selected screen."))
            .child(div().flex().flex_wrap().gap_3()
                .child(button("zero", "Zero tracker", ButtonKind::Primary, p)
                    .disabled(selected.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(monitor) = this.selection.read(cx).selected_key() {
                            crate::views::zero::open(monitor, cx);
                        }
                    })))
                .child(button("pair-tracker", "Pair tracker", ButtonKind::Secondary, p)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(HomeEvent::OpenPairing))))
                .child(button("manage-devices", "All devices", ButtonKind::Ghost, p)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(HomeEvent::OpenDevices)))));
        if selected.is_none() {
            panel = panel.child(
                div()
                    .text_sm()
                    .text_color(p.warning_ink)
                    .child("Connect and select a display before calibration or zeroing."),
            );
        }
        if let Some(error) = calibration_error {
            panel = panel
                .child(ui::alert(AlertKind::Warning, p, div().child(error)))
                .child(
                    button(
                        "retry-calibrations",
                        "Check again",
                        ButtonKind::Secondary,
                        p,
                    )
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.workspace
                            .update(cx, crate::workspace::Workspace::refresh_calibrations);
                    })),
                );
        }
        panel.into_any_element()
    }
}
