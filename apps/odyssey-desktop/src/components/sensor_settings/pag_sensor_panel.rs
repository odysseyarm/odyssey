use dioxus::prelude::*;

use super::field_issue::visible_issue;
use super::number_field::NumberField;
use super::pag_state::{
    PAG_AREA_MAX, PAG_AREA_MIN, PAG_EXPOSURE, PAG_FPS, PAG_GAIN, PAG_LIGHT, PagState,
    exposure_hint, pag_validate,
};
use super::styles::{FIELD_GRID_CLASS, SECTION_HEADING_CLASS};

#[component]
pub(super) fn PagSensorPanel(state: Signal<PagState>, show_issues: bool) -> Element {
    let issues = pag_validate(&state.read()).err().unwrap_or_default();
    let issue = |field: &str| visible_issue(&issues, field, show_issues);

    let exposure_hint = exposure_hint(&state.read().exposure_us);

    rsx! {
        div {
            class: "space-y-2",
            h4 {
                class: SECTION_HEADING_CLASS,
                "Sensor"
            }
            div {
                class: FIELD_GRID_CLASS,

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
