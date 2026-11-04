use dioxus::logger::tracing;
use dioxus::prelude::*;
use futures::StreamExt as _;
use odyssey_hub_common::device::{Device, DeviceCapabilities};

use super::field_issue::{invalid_status, issue_for};
use super::general_config::{general_validate, write_general_config};
use super::number_field::NumberField;
use super::op_status::{OpStatus, StatusLine, rpc_failure};
use super::pag_sensor_panel::PagSensorPanel;
use super::pag_state::{
    PagState, pag_apply_to_device, pag_load_defaults, pag_load_from_device, pag_validate,
};
use super::paj_sensor_panel::PajSensorPanel;
use super::paj_state::{
    PajState, paj_apply_to_device, paj_load_defaults, paj_load_from_device, paj_validate,
};
use super::product_ids::{PID_ATS_LITE, PID_ATS_PRO, PID_ATS_VM};
use crate::hub::HubContext;

// Disabled styling matters here: the write buttons go dead whenever a field is
// invalid, and a button that looks live but ignores clicks reads as a bug.
pub(super) const ACTION_PRIMARY_CLASS: &str =
    "btn-primary text-xs px-2 py-1 disabled:opacity-50 disabled:cursor-not-allowed";

pub(super) const ACTION_SECONDARY_CLASS: &str =
    "btn-secondary text-xs px-2 py-1 disabled:opacity-50 disabled:cursor-not-allowed";

/// What the settings load reads: the device as it is now and whether its sensors
/// can be tuned, which depends on the events link being up.
#[derive(Clone, PartialEq)]
pub(super) struct LoadTarget {
    pub(super) device: Device,
    pub(super) can_tune_sensors: bool,
}

#[component]
pub(crate) fn SensorSettings(hub: Signal<HubContext>, device: Device) -> Element {
    let pid = device.product_id;
    let is_paj = pid == PID_ATS_VM || pid == PID_ATS_LITE;
    let is_pag = pid == PID_ATS_PRO;
    let has_control = device.capabilities.contains(DeviceCapabilities::CONTROL);
    let has_events = device.capabilities.contains(DeviceCapabilities::EVENTS);
    // ReadConfig/WriteConfig/FlashSettings fall back to the control plane, so the
    // general settings work in BLE mode too. Register access needs the events
    // transport and the hub rejects it outright when that is missing, so the
    // sensor panels stay hidden instead of failing on every read.
    let should_render = (is_paj || is_pag) && (has_control || has_events);
    let can_tune_sensors = has_events;

    // All hooks must be called unconditionally before any early returns.
    let mut status = use_signal(OpStatus::default);
    let mut loading = use_signal(|| false);
    // Empty fields before the first read are "not loaded yet", not "invalid";
    // marking them red on first paint is noise the user cannot act on.
    let mut hydrated = use_signal(|| false);

    // General config state
    let mut impact_threshold = use_signal(String::new);
    let mut suppress_ms = use_signal(String::new);
    // Shot delay (stored on PC, not device)
    let mut shot_delay = use_signal(String::new);

    // PAJ state signals (NF and WF)
    let mut paj_nf_state = use_signal(PajState::default);
    let mut paj_wf_state = use_signal(PajState::default);
    // PAG state signal
    let mut pag_state = use_signal(PagState::default);

    // Tracks the open/closed state of the Device Settings section.
    let mut is_open = use_signal(|| false);

    // Coroutine: receives () each time settings should be loaded from device.
    // Triggered on section expand, the Reload button, and when the sensor panels
    // first become available.
    //
    // The closure runs once, so it reads the device and the sensor capability
    // through a signal rather than capturing them: both change while the
    // component stays mounted, since a unit whose Bluetooth events link attaches
    // after the first render only gains its sensor panels then.
    let mut load_target = use_signal(|| LoadTarget {
        device: device.clone(),
        can_tune_sensors,
    });
    let load_co = use_coroutine(move |mut rx: UnboundedReceiver<()>| async move {
        while rx.next().await.is_some() {
            loading.set(true);
            status.set(OpStatus::Busy("Reading from device...".into()));
            let hub_ctx = hub.peek().clone();
            let load_target = load_target.peek().clone();
            let device = load_target.device.clone();
            // Keep the first failure. A later read succeeding does not make the
            // form trustworthy, and reporting "Loaded" over a failed read is how
            // stale values end up being flashed back onto the device.
            let mut failure: Option<OpStatus> = None;

            match hub_ctx
                .client
                .peek()
                .clone()
                .read_config(device.clone(), 0)
                .await
            {
                Ok(v) => impact_threshold.set(v.to_string()),
                Err(e) => {
                    tracing::error!("reading impact threshold: {e:#}");
                    failure.get_or_insert(rpc_failure("read the impact threshold", &e));
                }
            }
            match hub_ctx
                .client
                .peek()
                .clone()
                .read_config(device.clone(), 1)
                .await
            {
                Ok(v) => suppress_ms.set(v.to_string()),
                Err(e) => {
                    tracing::error!("reading suppress time: {e:#}");
                    failure.get_or_insert(rpc_failure("read the suppress time", &e));
                }
            }
            match hub_ctx
                .client
                .peek()
                .clone()
                .get_shot_delay(device.clone())
                .await
            {
                Ok(v) => shot_delay.set(v.to_string()),
                Err(e) => {
                    tracing::error!("reading shot delay: {e:#}");
                    failure.get_or_insert(rpc_failure("read the shot delay", &e));
                }
            }

            if load_target.can_tune_sensors {
                let result = if is_paj {
                    match (
                        paj_load_from_device(&hub_ctx, &device, 0).await,
                        paj_load_from_device(&hub_ctx, &device, 1).await,
                    ) {
                        (Ok(nf), Ok(wf)) => {
                            paj_nf_state.set(nf);
                            paj_wf_state.set(wf);
                            Ok(())
                        }
                        (Err(e), _) | (_, Err(e)) => Err(e),
                    }
                } else {
                    pag_load_from_device(&hub_ctx, &device)
                        .await
                        .map(|s| pag_state.set(s))
                };
                if let Err(e) = result {
                    tracing::error!("reading sensor settings: {e:#}");
                    failure.get_or_insert(rpc_failure("read the sensor settings", &e));
                }
            }

            hydrated.set(true);
            status.set(failure.unwrap_or(OpStatus::Done("Loaded from device".into())));
            loading.set(false);
        }
    });

    // Keep the load target current, and fill the sensor panels as soon as they
    // appear: they render empty until something loads them, and a form that was
    // loaded before the events link came up never asked for them.
    {
        let device = device.clone();
        use_effect(use_reactive!(|(device, can_tune_sensors)| {
            let panels_appeared = !load_target.peek().can_tune_sensors && can_tune_sensors;
            load_target.set(LoadTarget {
                device,
                can_tune_sensors,
            });
            if panels_appeared && *is_open.peek() {
                load_co.send(());
            }
        }));
    }

    // Guards applied after all hooks have been called.
    if !should_render {
        return rsx! {};
    }

    let general_issues = general_validate(
        &impact_threshold.read(),
        &suppress_ms.read(),
        Some(&shot_delay.read()),
    )
    .err()
    .unwrap_or_default();
    let sensor_issue_count = if !can_tune_sensors {
        0
    } else if is_paj {
        paj_validate(&paj_nf_state.read())
            .err()
            .map_or(0, |v| v.len())
            + paj_validate(&paj_wf_state.read())
                .err()
                .map_or(0, |v| v.len())
    } else {
        pag_validate(&pag_state.read()).err().map_or(0, |v| v.len())
    };
    let has_invalid = !general_issues.is_empty() || sensor_issue_count > 0;
    let busy = *loading.read();
    // Writes stay blocked on invalid input either way; only the inline markers
    // wait for the first load.
    let show_issues = hydrated();
    let general_issue = |field: &str| {
        show_issues
            .then(|| issue_for(&general_issues, field).map(str::to_string))
            .flatten()
    };

    let reload = move |_| {
        load_co.send(());
    };

    // One code path for both write buttons: `persist` adds the flash step, so
    // the two cannot drift apart in what they send to the device.
    let device_for_write = device.clone();
    let write_all = std::sync::Arc::new(move |persist: bool| {
        let device = device_for_write.clone();
        async move {
            loading.set(true);
            status.set(OpStatus::Busy(
                if persist {
                    "Saving to device..."
                } else {
                    "Applying..."
                }
                .into(),
            ));
            let hub_ctx = hub.peek().clone();

            let general = match general_validate(
                &impact_threshold.peek(),
                &suppress_ms.peek(),
                Some(&shot_delay.peek()),
            ) {
                Ok(values) => values,
                Err(issues) => {
                    status.set(invalid_status(&issues));
                    loading.set(false);
                    return;
                }
            };
            if let Err(e) = write_general_config(&hub_ctx, &device, general).await {
                tracing::error!("writing general config: {e:#}");
                status.set(rpc_failure("write the general settings", &e));
                loading.set(false);
                return;
            }

            let sensors: Result<(), OpStatus> = async {
                if !can_tune_sensors {
                    return Ok(());
                }
                if is_paj {
                    for (port, state) in [(0u8, paj_nf_state), (1u8, paj_wf_state)] {
                        let values = paj_validate(&state.peek()).map_err(|i| invalid_status(&i))?;
                        paj_apply_to_device(&hub_ctx, &device, port, &values)
                            .await
                            .map_err(|e| {
                                tracing::error!("writing sensor settings: {e:#}");
                                rpc_failure("write the sensor settings", &e)
                            })?;
                    }
                } else {
                    let values = pag_validate(&pag_state.peek()).map_err(|i| invalid_status(&i))?;
                    pag_apply_to_device(&hub_ctx, &device, &values)
                        .await
                        .map_err(|e| {
                            tracing::error!("writing sensor settings: {e:#}");
                            rpc_failure("write the sensor settings", &e)
                        })?;
                }
                Ok(())
            }
            .await;
            if let Err(failed) = sensors {
                status.set(failed);
                loading.set(false);
                return;
            }

            // Flash last: it persists whatever the device currently holds, so it
            // must only run once every write above has succeeded.
            if persist {
                if let Err(e) = hub_ctx
                    .client
                    .peek()
                    .clone()
                    .flash_settings(device.clone())
                    .await
                {
                    tracing::error!("flashing settings: {e:#}");
                    status.set(rpc_failure("save the settings to the device", &e));
                    loading.set(false);
                    return;
                }
                status.set(OpStatus::Done("Saved to the device".into()));
            } else {
                status.set(OpStatus::Done(
                    "Applied for this session. Power cycling restores the saved values.".into(),
                ));
            }
            loading.set(false);
        }
    });

    let restore_defaults = move |_| {
        impact_threshold.set("25".into());
        suppress_ms.set("25".into());
        if can_tune_sensors {
            if is_paj {
                paj_nf_state.set(paj_load_defaults(0));
                paj_wf_state.set(paj_load_defaults(1));
            } else {
                pag_state.set(pag_load_defaults());
            }
        }
        status.set(OpStatus::Done(
            "Defaults filled in. Nothing reaches the device until you save.".into(),
        ));
    };

    rsx! {
        details {
            class: "mt-2",
            open: *is_open.read(),
            summary {
                class: "text-sm font-medium text-gray-700 dark:text-gray-300 cursor-pointer \
                        hover:text-blue-600 dark:hover:text-blue-400 select-none",
                onclick: move |_| {
                    let opening = !*is_open.peek();
                    is_open.set(opening);
                    if opening {
                        load_co.send(());
                    }
                },
                "Device Settings"
            }
            div {
                class: "mt-2 space-y-3 p-3 bg-gray-100 dark:bg-gray-700 rounded-lg",

                // Action buttons
                div {
                    class: "flex items-center gap-2 flex-wrap",
                    button {
                        class: ACTION_PRIMARY_CLASS,
                        disabled: busy || has_invalid,
                        title: "Write these values and keep them across power cycles",
                        onclick: { let write_all = write_all.clone(); move |_| write_all(true) },
                        "Save to device"
                    }
                    button {
                        class: ACTION_SECONDARY_CLASS,
                        disabled: busy || has_invalid,
                        title: "Write these values for this session only",
                        onclick: { let write_all = write_all.clone(); move |_| write_all(false) },
                        "Try without saving"
                    }
                    button {
                        class: ACTION_SECONDARY_CLASS,
                        disabled: busy,
                        title: "Discard edits and re-read the device",
                        onclick: reload,
                        "Reload"
                    }
                    button {
                        class: ACTION_SECONDARY_CLASS,
                        disabled: busy,
                        title: "Fill the form with factory values; nothing is written yet",
                        onclick: restore_defaults,
                        "Restore defaults"
                    }
                    StatusLine { status: status() }
                }

                // General settings
                div {
                    class: "space-y-2",
                    h4 {
                        class: "text-sm font-semibold text-gray-800 dark:text-gray-200",
                        "Impact detection"
                    }
                    div {
                        class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center",

                        NumberField {
                            label: "Impact threshold",
                            value: impact_threshold(),
                            issue: general_issue("Impact threshold"),
                            oninput: move |v| impact_threshold.set(v),
                        }
                        NumberField {
                            label: "Suppress",
                            value: suppress_ms(),
                            hint: "ms",
                            issue: general_issue("Suppress"),
                            oninput: move |v| suppress_ms.set(v),
                        }
                    }
                }

                // Shot delay lives on this computer, not on the device.
                div {
                    class: "space-y-2",
                    h4 {
                        class: "text-sm font-semibold text-gray-800 dark:text-gray-200",
                        "This computer"
                    }
                    div {
                        class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center",

                        NumberField {
                            label: "Shot delay",
                            value: shot_delay(),
                            hint: "ms",
                            issue: general_issue("Shot delay"),
                            oninput: move |v| shot_delay.set(v),
                        }
                    }
                    p {
                        class: "text-xs text-gray-500 dark:text-gray-400",
                        "Stored on this computer, not on the device."
                    }
                }

                // Sensor panels
                if can_tune_sensors {
                    if is_paj {
                        PajSensorPanel {
                            label: "Near Field",
                            state: paj_nf_state,
                            show_issues: show_issues,
                        }
                        PajSensorPanel {
                            label: "Wide Field",
                            state: paj_wf_state,
                            show_issues: show_issues,
                        }
                    }
                    if is_pag {
                        PagSensorPanel { state: pag_state, show_issues: show_issues }
                    }
                } else {
                    div {
                        class: "space-y-1 border-t border-gray-300 dark:border-gray-600 pt-2",
                        h4 {
                            class: "text-sm font-semibold text-gray-800 dark:text-gray-200",
                            "Sensor tuning"
                        }
                        p {
                            class: "text-xs text-gray-500 dark:text-gray-400",
                            "Available over USB only. Set Transport mode to USB in the \
                             Connection section, then reopen this panel."
                        }
                    }
                }
            }
        }
    }
}
