//! Setup's "measure the size by hand" helper: a tape measure on a line the
//! screen draws, or a credit card against a frame it draws.
//!
//! It sits under the size question so anyone who doubts the size sees it, and
//! asks for more when the display reported nothing usable. The controls live
//! here; the line or frame is shown by the pattern window on the monitor being
//! measured (see `PatternMode::Ruler` and `Card`).

use dioxus::prelude::*;

use super::kind::DisplayKind;
use super::reference::{self, Agreement, CARD_STEP_PX, Method, Unit};
use super::ui::{PRIMARY_BUTTON, SECONDARY_BUTTON};
use super::wizard::{Helper, Wizard};
use crate::components::wizard::{Icon, IconView};

const LINK: &str = "text-base font-medium text-blue-700 underline underline-offset-2 hover:text-blue-900 focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:text-blue-300 dark:hover:text-blue-100";
const SEGMENT_ON: &str = "bg-blue-600 text-white";
const SEGMENT_OFF: &str = "bg-white text-gray-800 hover:bg-gray-100 dark:bg-gray-800 dark:text-gray-100 dark:hover:bg-gray-700";

#[component]
pub fn ReferenceHelper(wizard: Wizard) -> Element {
    let helper = wizard.helper.read().clone();
    match helper {
        Some(helper) => rsx! { OpenHelper { wizard, helper } },
        None if wizard.reference().is_some() => rsx! { Measured { wizard } },
        None => rsx! { Prompt { wizard } },
    }
}

/// "123.4 cm (48.6 in) wide": a picture's width in both units a tech might think in.
fn width_words(size_m: [f64; 2]) -> String {
    format!(
        "{:.1} cm ({:.1} in) wide",
        size_m[0] * 100.0,
        size_m[0] / 0.0254
    )
}

/// How a hand measurement compares with the size the display reported.
fn agreement_note(wizard: Wizard, size_m: [f64; 2]) -> Option<(String, bool)> {
    let gap = reference::size_gap(size_m, wizard.panel_size_m()?);
    let percent = gap * 100.0;
    Some(match reference::agreement(gap) {
        Agreement::Close => (
            format!("Within {percent:.1}% of what the TV reports."),
            false,
        ),
        Agreement::Differs => (
            format!(
                "{percent:.1}% from what the TV reports. That's expected if the TV crops its picture, since you measured the whole picture, and displays are often a little off too."
            ),
            false,
        ),
        Agreement::FarOff => (
            format!(
                "{percent:.0}% from what the TV reports. A TV that crops a lot of its picture can do that. If it doesn't, check the unit and where you measured from."
            ),
            true,
        ),
    })
}

/// The invitation: always there for a flat screen or a projector, louder when
/// the display told us nothing usable.
#[component]
fn Prompt(wizard: Wizard) -> Element {
    let Some(finding) = wizard.finding.read().clone() else {
        return rsx! {};
    };
    let projected = wizard.kind() == DisplayKind::Projector;
    let silent = projected || !finding.is_found();
    let (title, detail) = if projected {
        (
            "Know the picture's size exactly?",
            "The camera measures a projected picture. A tape measure or a credit card takes about a minute and gives it an exact size.",
        )
    } else if silent {
        (
            "Measure the size by hand for the best result",
            "The TV didn't report a usable size, so the camera would measure it. A tape measure or a credit card takes about a minute and is usually more exact.",
        )
    } else {
        (
            "Want to be sure of the size?",
            "The TV reports its size, but displays are sometimes a little off. A tape measure or a credit card takes about a minute and checks it.",
        )
    };
    let frame = if silent {
        "border-amber-300 bg-amber-50 dark:border-amber-700 dark:bg-amber-950/60"
    } else {
        "border-gray-200 bg-gray-50 dark:border-gray-600 dark:bg-gray-800/60"
    };
    rsx! {
        div {
            class: "flex flex-col gap-3 rounded-xl border-2 p-4 {frame}",
            div {
                class: "flex items-start gap-3",
                IconView { icon: Icon::Bulb, class: "mt-0.5 size-6 shrink-0 text-sky-600 dark:text-sky-300".to_string() }
                div {
                    class: "flex flex-col gap-1",
                    p { class: "text-lg font-medium text-gray-900 dark:text-white", "{title}" }
                    p { class: "text-base text-gray-600 dark:text-gray-300", "{detail}" }
                }
            }
            div {
                class: "flex flex-wrap gap-3",
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.open_helper(Method::Tape),
                    "Use a tape measure"
                }
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.open_helper(Method::Card),
                    "Use a credit card"
                }
            }
            p {
                class: "text-sm text-gray-600 dark:text-gray-300",
                "The tape is the most exact; the card is quicker."
            }
        }
    }
}

/// The result in use, with a way to redo or drop it.
#[component]
fn Measured(wizard: Wizard) -> Element {
    let (Some(reading), Some(key)) = (wizard.reference(), wizard.selected_monitor()) else {
        return rsx! {};
    };
    let size = reading.size_m(key.width, key.height);
    let how = match reading.method {
        Method::Tape => "tape measure",
        Method::Card => "credit card",
    };
    let note = agreement_note(wizard, size);
    rsx! {
        div {
            class: "flex flex-col gap-2 rounded-xl border-2 border-emerald-300 bg-emerald-50 p-4 dark:border-emerald-700 dark:bg-emerald-950/50",
            div {
                class: "flex items-start gap-3",
                IconView { icon: Icon::Check, class: "mt-0.5 size-6 shrink-0 text-emerald-600 dark:text-emerald-400".to_string() }
                div {
                    class: "flex flex-col gap-1",
                    p {
                        class: "text-lg font-medium text-gray-900 dark:text-white",
                        "Using your {how} measurement: {width_words(size)}."
                    }
                    if let Some((text, warn)) = note {
                        p {
                            class: if warn { "text-base text-amber-800 dark:text-amber-200" } else { "text-base text-gray-600 dark:text-gray-300" },
                            "{text}"
                        }
                    }
                }
            }
            div {
                class: "flex flex-wrap gap-4",
                button {
                    class: LINK,
                    r#type: "button",
                    onclick: move |_| wizard.open_helper(reading.method),
                    "Measure again"
                }
                button {
                    class: LINK,
                    r#type: "button",
                    onclick: move |_| wizard.clear_reference(),
                    "Don't use it"
                }
            }
        }
    }
}

#[component]
fn OpenHelper(wizard: Wizard, helper: Helper) -> Element {
    let method = helper.method;
    let reading = wizard.helper_reading();
    let typed = !helper.tape_text.trim().is_empty();
    let usable = matches!(reading, Some(Ok(_))) && (method == Method::Card || typed);
    let key = wizard.selected_monitor();
    let preview = match (&reading, &key) {
        (Some(Ok(r)), Some(key)) if method == Method::Card || typed => {
            let size = r.size_m(key.width, key.height);
            Some((width_words(size), agreement_note(wizard, size)))
        }
        _ => None,
    };
    let error = match (&reading, method, typed) {
        (Some(Err(e)), Method::Tape, true) => Some(e.to_string()),
        _ => None,
    };
    rsx! {
        div {
            class: "flex flex-col gap-4 rounded-xl border-2 border-blue-300 bg-blue-50 p-4 dark:border-blue-700 dark:bg-blue-950/40",
            div {
                class: "flex flex-wrap items-center justify-between gap-3",
                p { class: "text-lg font-semibold text-gray-900 dark:text-white", "Measure the size by hand" }
                div {
                    role: "radiogroup",
                    aria_label: "How to measure",
                    class: "inline-flex overflow-hidden rounded-lg border border-gray-300 dark:border-gray-500",
                    for (option, label) in [(Method::Tape, "Tape measure"), (Method::Card, "Credit card")] {
                        {
                            let tone = if option == method { SEGMENT_ON } else { SEGMENT_OFF };
                            rsx! {
                                button {
                                    key: "{label}",
                                    r#type: "button",
                                    role: "radio",
                                    aria_checked: option == method,
                                    class: "px-4 py-2 text-base font-medium focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 {tone}",
                                    onclick: move |_| wizard.set_helper_method(option),
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }

            match method {
                Method::Tape => rsx! { TapeSteps { wizard, helper: helper.clone() } },
                Method::Card => rsx! { CardSteps { wizard, helper: helper.clone() } },
            }

            if let Some((width, note)) = preview {
                div {
                    class: "flex flex-col gap-1",
                    p { class: "text-base text-gray-900 dark:text-white", "That makes the picture {width}." }
                    if let Some((text, warn)) = note {
                        p {
                            class: if warn { "text-base text-amber-800 dark:text-amber-200" } else { "text-sm text-gray-600 dark:text-gray-300" },
                            "{text}"
                        }
                    }
                }
            }
            if let Some(error) = error {
                p { role: "alert", class: "text-base text-red-700 dark:text-red-300", "{error}" }
            }

            div {
                class: "flex flex-wrap items-center gap-3",
                button {
                    class: PRIMARY_BUTTON,
                    r#type: "button",
                    disabled: !usable,
                    onclick: move |_| wizard.apply_helper(),
                    "Use this size"
                }
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.cancel_helper(),
                    "Cancel"
                }
            }
        }
    }
}

#[component]
fn TapeSteps(wizard: Wizard, helper: Helper) -> Element {
    let unit = helper.tape_unit;
    rsx! {
        div {
            class: "flex flex-col gap-3",
            ol {
                class: "flex list-decimal flex-col gap-1 pl-6 text-base text-gray-800 dark:text-gray-100",
                li { "Look at the screen: it shows a line with a tick at each end." }
                li { "Lay the tape along the line and read the length from the middle of one tick to the middle of the other." }
                li { "Type what you read." }
            }
            div {
                class: "flex flex-wrap items-center gap-3",
                input {
                    id: "tape-length",
                    r#type: "text",
                    inputmode: "decimal",
                    autocomplete: "off",
                    aria_label: "Length read from the tape",
                    placeholder: if unit == Unit::Inches { "47 5/8" } else { "121.0" },
                    class: "w-40 rounded-lg border border-gray-300 bg-white px-3 py-2 text-lg text-gray-900 focus:border-blue-500 focus:outline-none focus:ring-4 focus:ring-blue-200 dark:border-gray-500 dark:bg-gray-800 dark:text-white dark:focus:ring-blue-900",
                    value: "{helper.tape_text}",
                    oninput: move |e| wizard.set_tape(e.value(), unit),
                }
                div {
                    role: "radiogroup",
                    aria_label: "Unit of the number you typed",
                    class: "inline-flex overflow-hidden rounded-lg border border-gray-300 dark:border-gray-500",
                    for (option, label) in [(Unit::Inches, "in"), (Unit::Centimeters, "cm"), (Unit::Millimeters, "mm")] {
                        {
                            let tone = if option == unit { SEGMENT_ON } else { SEGMENT_OFF };
                            let text = helper.tape_text.clone();
                            rsx! {
                                button {
                                    key: "{label}",
                                    r#type: "button",
                                    role: "radio",
                                    aria_checked: option == unit,
                                    class: "px-3 py-2 text-base font-medium focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 {tone}",
                                    onclick: move |_| wizard.set_tape(text.clone(), option),
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CardSteps(wizard: Wizard, helper: Helper) -> Element {
    let (lo, hi) = helper.card_bounds;
    let px = helper.card_px;
    rsx! {
        div {
            class: "flex flex-col gap-3",
            ol {
                class: "flex list-decimal flex-col gap-1 pl-6 text-base text-gray-800 dark:text-gray-100",
                li { "Take any bank card, driver's licence or ID card." }
                li { "Hold it flat against the glass inside the white frame on the screen, and look straight at it." }
                li { "Nudge the frame until it hugs the card: no dark gap, and none of the frame covered." }
            }
            div {
                class: "flex flex-wrap items-center gap-3",
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    aria_label: "Smaller frame",
                    onclick: move |_| wizard.set_card_px(px - CARD_STEP_PX),
                    "\u{2212}"
                }
                input {
                    r#type: "range",
                    aria_label: "Frame size",
                    class: "w-64 max-w-full",
                    min: "{lo}",
                    max: "{hi}",
                    step: "{CARD_STEP_PX}",
                    value: "{px}",
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<f64>() {
                            wizard.set_card_px(v);
                        }
                    },
                }
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    aria_label: "Larger frame",
                    onclick: move |_| wizard.set_card_px(px + CARD_STEP_PX),
                    "+"
                }
                span { class: "text-sm text-gray-600 dark:text-gray-300", "{px:.2} pixels wide" }
            }
        }
    }
}
