use dioxus::prelude::*;

use super::field_issue::visible_issue;
use super::gain_table::GAIN_TABLE;
use super::number_field::NumberField;
use super::paj_state::{
    PAJ_AREA_MAX, PAJ_AREA_MIN, PAJ_BRIGHTNESS, PAJ_EXPOSURE, PAJ_FRAME_PERIOD,
    PAJ_FRAME_SUBTRACTION, PAJ_GAIN, PAJ_MAX_OBJECTS, PAJ_NOISE, PAJ_OPERATION_MODE,
    PAJ_RESOLUTION_X, PAJ_RESOLUTION_Y, PajState, exposure_hint, frame_period_hint, paj_validate,
};
use super::styles::{
    FIELD_CLASS, FIELD_GRID_CLASS, INVALID_FIELD_CLASS, LABEL_CLASS, SECTION_HEADING_CLASS,
};

#[component]
pub(super) fn PajSensorPanel(label: String, state: Signal<PajState>, show_issues: bool) -> Element {
    let issues = paj_validate(&state.read()).err().unwrap_or_default();
    let issue = |field: &str| visible_issue(&issues, field, show_issues);

    let exposure_hint = exposure_hint(&state.read().exposure_time);
    let frame_hint = frame_period_hint(&state.read().frame_period);

    rsx! {
        div {
            class: "space-y-2",
            h4 {
                class: SECTION_HEADING_CLASS,
                {label}
            }
            div {
                class: FIELD_GRID_CLASS,

                NumberField {
                    label: PAJ_BRIGHTNESS,
                    value: state.read().brightness_threshold.clone(),
                    issue: issue(PAJ_BRIGHTNESS),
                    oninput: move |v| state.write().brightness_threshold = v,
                }
                NumberField {
                    label: PAJ_NOISE,
                    value: state.read().noise_threshold.clone(),
                    issue: issue(PAJ_NOISE),
                    oninput: move |v| state.write().noise_threshold = v,
                }
                NumberField {
                    label: PAJ_AREA_MIN,
                    value: state.read().area_threshold_min.clone(),
                    hint: "px",
                    issue: issue(PAJ_AREA_MIN),
                    oninput: move |v| state.write().area_threshold_min = v,
                }
                NumberField {
                    label: PAJ_AREA_MAX,
                    value: state.read().area_threshold_max.clone(),
                    hint: "px",
                    issue: issue(PAJ_AREA_MAX),
                    oninput: move |v| state.write().area_threshold_max = v,
                }
                NumberField {
                    label: PAJ_MAX_OBJECTS,
                    value: state.read().max_object_cnt.clone(),
                    issue: issue(PAJ_MAX_OBJECTS),
                    oninput: move |v| state.write().max_object_cnt = v,
                }

                label { class: LABEL_CLASS, {PAJ_OPERATION_MODE} }
                div {
                    select {
                        class: if issue(PAJ_OPERATION_MODE).is_some() { INVALID_FIELD_CLASS } else { FIELD_CLASS },
                        value: "{state.read().operation_mode}",
                        onchange: move |e| state.write().operation_mode = e.value(),
                        option { value: "0", "Normal" }
                        option { value: "1", "Tracking" }
                    }
                    if let Some(message) = issue(PAJ_OPERATION_MODE) {
                        p { class: "text-xs text-red-600 dark:text-red-400", {message} }
                    }
                }

                NumberField {
                    label: PAJ_EXPOSURE,
                    value: state.read().exposure_time.clone(),
                    hint: exposure_hint,
                    issue: issue(PAJ_EXPOSURE),
                    oninput: move |v| state.write().exposure_time = v,
                }
                NumberField {
                    label: PAJ_FRAME_PERIOD,
                    value: state.read().frame_period.clone(),
                    hint: frame_hint,
                    issue: issue(PAJ_FRAME_PERIOD),
                    oninput: move |v| state.write().frame_period = v,
                }

                label { class: LABEL_CLASS, {PAJ_FRAME_SUBTRACTION} }
                div {
                    select {
                        class: if issue(PAJ_FRAME_SUBTRACTION).is_some() { INVALID_FIELD_CLASS } else { FIELD_CLASS },
                        value: "{state.read().frame_subtraction}",
                        onchange: move |e| state.write().frame_subtraction = e.value(),
                        option { value: "0", "Off" }
                        option { value: "1", "On" }
                    }
                    if let Some(message) = issue(PAJ_FRAME_SUBTRACTION) {
                        p { class: "text-xs text-red-600 dark:text-red-400", {message} }
                    }
                }

                label { class: LABEL_CLASS, {PAJ_GAIN} }
                div {
                    select {
                        class: if issue(PAJ_GAIN).is_some() { INVALID_FIELD_CLASS } else { FIELD_CLASS },
                        value: "{state.read().gain_index}",
                        onchange: move |e| state.write().gain_index = e.value(),
                        for (i, entry) in GAIN_TABLE.iter().enumerate() {
                            option { value: "{i}", "{entry.label}" }
                        }
                    }
                    if let Some(message) = issue(PAJ_GAIN) {
                        p { class: "text-xs text-red-600 dark:text-red-400", {message} }
                    }
                }

                NumberField {
                    label: PAJ_RESOLUTION_X,
                    value: state.read().resolution_x.clone(),
                    issue: issue(PAJ_RESOLUTION_X),
                    oninput: move |v| state.write().resolution_x = v,
                }
                NumberField {
                    label: PAJ_RESOLUTION_Y,
                    value: state.read().resolution_y.clone(),
                    issue: issue(PAJ_RESOLUTION_Y),
                    oninput: move |v| state.write().resolution_y = v,
                }
            }
        }
    }
}
