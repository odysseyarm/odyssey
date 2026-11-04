use dioxus::prelude::*;
use futures::StreamExt as _;
use odyssey_hub_common::device::{Device, DeviceCapabilities};

use super::field_issue::{FieldIssue, visible_issue};
use super::general_config::{IMPACT_THRESHOLD_FIELD, SHOT_DELAY_FIELD, SUPPRESS_FIELD};
use super::number_field::NumberField;
use super::op_status::{OpStatus, StatusLine};
use super::pag_sensor_panel::PagSensorPanel;
use super::paj_sensor_panel::PajSensorPanel;
use super::product_ids::SensorFamily;
use super::settings_form::{LoadTarget, SettingsForm, use_settings_form};
use super::styles::{
    ACTION_PRIMARY_CLASS, ACTION_SECONDARY_CLASS, DISCLOSURE_SUMMARY_CLASS, FIELD_GRID_CLASS,
    HELP_TEXT_CLASS, SECTION_HEADING_CLASS,
};
use crate::hub::HubContext;

enum SettingsOperation {
    Load,
    LoadSensors,
    Write(bool),
    RestoreDefaults,
}

#[component]
pub(crate) fn SensorSettings(hub: Signal<HubContext>, device: Device) -> Element {
    let family = SensorFamily::for_product(device.product_id);
    let has_control = device.capabilities.contains(DeviceCapabilities::CONTROL);
    let has_events = device.capabilities.contains(DeviceCapabilities::EVENTS);
    // General settings can use the control plane; register access needs a live
    // events connection, whether it arrives over USB or Bluetooth.
    let should_render = family.is_some() && (has_control || has_events);
    let can_tune_sensors = has_events && device.events_connected;
    let tunable_sensors = family.filter(|_| can_tune_sensors);

    // Hooks must all run, in the same order, before any early return.
    let form = use_settings_form();
    let mut is_open = use_signal(|| false);

    // Serialize all form operations so a queued reload cannot race a save or
    // overwrite defaults that the user filled after requesting that reload.
    let mut load_target = use_signal(|| LoadTarget {
        device: device.clone(),
        can_tune_sensors,
    });
    let load_co = use_coroutine(
        move |mut rx: UnboundedReceiver<SettingsOperation>| async move {
            while let Some(operation) = rx.next().await {
                let target = load_target.peek().clone();
                match operation {
                    SettingsOperation::Load => form.load(hub, &target, false).await,
                    SettingsOperation::LoadSensors => form.load(hub, &target, true).await,
                    SettingsOperation::Write(persist) => {
                        form.write_all(hub, &target, persist).await;
                    }
                    SettingsOperation::RestoreDefaults => {
                        form.restore_defaults(target.tunable_sensors());
                    }
                }
            }
        },
    );

    // Keep the load target current, and fill the sensor panels as soon as they
    // appear: they render empty until something loads them, and a form that was
    // loaded before the events link came up never asked for them.
    // Only load the newly available sensors; preserve unsaved general edits.
    {
        let device = device.clone();
        use_effect(use_reactive!(|(device, can_tune_sensors)| {
            let panels_appeared = can_tune_sensors
                && (!load_target.peek().can_tune_sensors
                    || load_target.peek().device.product_id != device.product_id);
            load_target.set(LoadTarget {
                device,
                can_tune_sensors,
            });
            if panels_appeared && *is_open.peek() {
                load_co.send(SettingsOperation::LoadSensors);
            }
        }));
    }

    let write_all = use_callback(move |persist| {
        load_co.send(SettingsOperation::Write(persist));
    });

    if !should_render {
        return rsx! {};
    }

    let general_issues = form.general_issues();
    let has_invalid = form.has_invalid_input(tunable_sensors);
    let busy = *form.loading.read();
    // Writes stay blocked on invalid input either way; only the inline markers
    // wait for the first load.
    let show_issues = *form.hydrated.read();

    rsx! {
        details {
            class: "mt-2",
            open: *is_open.read(),
            summary {
                class: DISCLOSURE_SUMMARY_CLASS,
                onclick: move |_| {
                    let opening = !*is_open.peek();
                    is_open.set(opening);
                    if opening {
                        load_co.send(SettingsOperation::Load);
                    }
                },
                "Device Settings"
            }
            div {
                class: "mt-2 space-y-3 p-3 bg-gray-100 dark:bg-gray-700 rounded-lg",

                SettingsActions {
                    busy,
                    has_invalid,
                    status: (form.status)(),
                    on_write: write_all,
                    on_reload: move |()| load_co.send(SettingsOperation::Load),
                    on_restore: move |()| load_co.send(SettingsOperation::RestoreDefaults),
                }
                ImpactDetectionSection {
                    impact_threshold: form.impact_threshold,
                    suppress_ms: form.suppress_ms,
                    issues: general_issues.clone(),
                    show_issues,
                }
                ShotDelaySection {
                    shot_delay: form.shot_delay,
                    issues: general_issues,
                    show_issues,
                }
                SensorTuning { form, sensors: tunable_sensors, show_issues }
            }
        }
    }
}

/// The write/reload/defaults buttons and the status of the last operation.
#[component]
fn SettingsActions(
    busy: bool,
    has_invalid: bool,
    status: OpStatus,
    on_write: EventHandler<bool>,
    on_reload: EventHandler<()>,
    on_restore: EventHandler<()>,
) -> Element {
    rsx! {
        div {
            class: "flex items-center gap-2 flex-wrap",
            button {
                class: ACTION_PRIMARY_CLASS,
                disabled: busy || has_invalid,
                title: "Write these values and keep them across power cycles",
                onclick: move |_| on_write.call(true),
                "Save to device"
            }
            button {
                class: ACTION_SECONDARY_CLASS,
                disabled: busy || has_invalid,
                title: "Write these values for this session only",
                onclick: move |_| on_write.call(false),
                "Try without saving"
            }
            button {
                class: ACTION_SECONDARY_CLASS,
                disabled: busy,
                title: "Discard edits and re-read the device",
                onclick: move |_| on_reload.call(()),
                "Reload"
            }
            button {
                class: ACTION_SECONDARY_CLASS,
                disabled: busy,
                title: "Fill the form with factory values; nothing is written yet",
                onclick: move |_| on_restore.call(()),
                "Restore defaults"
            }
            StatusLine { status }
        }
    }
}

#[component]
fn ImpactDetectionSection(
    impact_threshold: Signal<String>,
    suppress_ms: Signal<String>,
    issues: Vec<FieldIssue>,
    show_issues: bool,
) -> Element {
    rsx! {
        div {
            class: "space-y-2",
            h4 {
                class: SECTION_HEADING_CLASS,
                "Impact detection"
            }
            div {
                class: FIELD_GRID_CLASS,

                NumberField {
                    label: IMPACT_THRESHOLD_FIELD,
                    value: impact_threshold(),
                    issue: visible_issue(&issues, IMPACT_THRESHOLD_FIELD, show_issues),
                    oninput: move |value| impact_threshold.set(value),
                }
                NumberField {
                    label: SUPPRESS_FIELD,
                    value: suppress_ms(),
                    hint: "ms",
                    issue: visible_issue(&issues, SUPPRESS_FIELD, show_issues),
                    oninput: move |value| suppress_ms.set(value),
                }
            }
        }
    }
}

/// Shot delay lives on this computer, not on the device.
#[component]
fn ShotDelaySection(
    shot_delay: Signal<String>,
    issues: Vec<FieldIssue>,
    show_issues: bool,
) -> Element {
    rsx! {
        div {
            class: "space-y-2",
            h4 {
                class: SECTION_HEADING_CLASS,
                "This computer"
            }
            div {
                class: FIELD_GRID_CLASS,

                NumberField {
                    label: SHOT_DELAY_FIELD,
                    value: shot_delay(),
                    hint: "ms",
                    issue: visible_issue(&issues, SHOT_DELAY_FIELD, show_issues),
                    oninput: move |value| shot_delay.set(value),
                }
            }
            p {
                class: HELP_TEXT_CLASS,
                "Stored on this computer, not on the device."
            }
        }
    }
}

/// The per-sensor panels, or a pointer to USB when register access is down.
#[component]
fn SensorTuning(form: SettingsForm, sensors: Option<SensorFamily>, show_issues: bool) -> Element {
    match sensors {
        Some(SensorFamily::Paj) => rsx! {
            PajSensorPanel {
                label: "Near Field",
                state: form.paj_near_field,
                show_issues,
            }
            PajSensorPanel {
                label: "Wide Field",
                state: form.paj_wide_field,
                show_issues,
            }
        },
        Some(SensorFamily::Pag) => rsx! {
            PagSensorPanel { state: form.pag, show_issues }
        },
        None => rsx! {
            div {
                class: "space-y-1 border-t border-gray-300 dark:border-gray-600 pt-2",
                h4 {
                    class: SECTION_HEADING_CLASS,
                    "Sensor tuning"
                }
                p {
                    class: HELP_TEXT_CLASS,
                    "Sensor tuning needs an active sensor data connection. Connect \
                     over USB or Bluetooth, then reload the settings."
                }
            }
        },
    }
}
