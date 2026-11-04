use dioxus::prelude::*;

use super::field_issue::issue_for;
use super::number_field::NumberField;
use super::pag_state::{
    PAG_AREA_MAX, PAG_AREA_MIN, PAG_EXPOSURE, PAG_FPS, PAG_GAIN, PAG_LIGHT, PagState, pag_validate,
};

#[component]
pub(super) fn PagSensorPanel(state: Signal<PagState>, show_issues: bool) -> Element {
    let issues = pag_validate(&state.read()).err().unwrap_or_default();
    let issue = |field: &str| {
        show_issues
            .then(|| issue_for(&issues, field).map(str::to_string))
            .flatten()
    };

    let exposure_hint = {
        let val: f64 = state.read().exposure_us.parse().unwrap_or(0.0);
        format!("{:.1} ms", val / 1000.0)
    };

    rsx! {
        div {
            class: "space-y-2",
            h4 {
                class: "text-sm font-semibold text-gray-800 dark:text-gray-200",
                "Sensor"
            }
            div {
                class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center",

                NumberField {
                    label: PAG_FPS,
                    value: state.read().fps.clone(),
                    hint: "fps",
                    issue: issue(PAG_FPS),
                    oninput: move |v| state.write().fps = v,
                }
                NumberField {
                    label: PAG_EXPOSURE,
                    value: state.read().exposure_us.clone(),
                    hint: "µs · {exposure_hint}",
                    issue: issue(PAG_EXPOSURE),
                    oninput: move |v| state.write().exposure_us = v,
                }
                NumberField {
                    label: PAG_GAIN,
                    value: state.read().gain.clone(),
                    issue: issue(PAG_GAIN),
                    oninput: move |v| state.write().gain = v,
                }
                NumberField {
                    label: PAG_LIGHT,
                    value: state.read().light_threshold.clone(),
                    issue: issue(PAG_LIGHT),
                    oninput: move |v| state.write().light_threshold = v,
                }
                NumberField {
                    label: PAG_AREA_MIN,
                    value: state.read().area_lower.clone(),
                    hint: "px",
                    issue: issue(PAG_AREA_MIN),
                    oninput: move |v| state.write().area_lower = v,
                }
                NumberField {
                    label: PAG_AREA_MAX,
                    value: state.read().area_upper.clone(),
                    hint: "px",
                    issue: issue(PAG_AREA_MAX),
                    oninput: move |v| state.write().area_upper = v,
                }
            }
        }
    }
}
