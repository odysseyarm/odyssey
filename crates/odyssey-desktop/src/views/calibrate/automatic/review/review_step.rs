//! Step 5: the result. The camera picture with the fitted screen outline and
//! marker positions, the measured size, a green/amber/red verdict, and Save.

use dioxus::prelude::*;
use odyssey_autocalib::session::{EdgeCheck, SizeCheck, SizeSource};

use super::notices::{
    DisagreementNotice, OverscanNotice, ReplaceConfirmation, SaveError, UsingMeasuredNotice,
};
use super::result_overlay::{ResultOverlay, outline_color};
use crate::components::wizard::{Icon, StatusBanner, Tone};
use crate::views::calibrate::automatic::attempt_button::SaveAttempt;
use crate::views::calibrate::automatic::hints::circle_text;
use crate::views::calibrate::automatic::kind::source_label;
use crate::views::calibrate::automatic::quality::{Verdict, assess};
use crate::views::calibrate::automatic::screens::screen_label;
use crate::views::calibrate::automatic::ui::{
    Details, Fact, PRIMARY_BUTTON, PRIMARY_ID, SECONDARY_BUTTON, StepPage, focus_primary,
};
use crate::views::calibrate::automatic::wizard::Wizard;

fn tone(verdict: Verdict) -> Tone {
    match verdict {
        Verdict::Good => Tone::Good,
        Verdict::Fair => Tone::Attention,
        Verdict::Poor => Tone::Bad,
    }
}

fn headline(verdict: Verdict) -> (Icon, &'static str) {
    match verdict {
        Verdict::Good => (Icon::Check, "Calibration looks great"),
        Verdict::Fair => (Icon::Warning, "Usable, but it could be better"),
        Verdict::Poor => (Icon::Cross, "This measurement isn't reliable"),
    }
}

/// The save and redo buttons, which swap places with the verdict: the
/// recommended action sits on the right. Neither action runs during a save.
#[component]
fn ReviewFooter(wizard: Wizard, poor: bool, saving: bool, confirming: bool) -> Element {
    let (save_class, redo_class) = if poor {
        (SECONDARY_BUTTON, PRIMARY_BUTTON)
    } else {
        (PRIMARY_BUTTON, SECONDARY_BUTTON)
    };
    let save_label = if saving {
        "Saving…"
    } else if poor {
        "Save anyway"
    } else {
        "Save"
    };
    let save_id = (!poor).then_some(PRIMARY_ID);
    let redo_id = poor.then_some(PRIMARY_ID);
    let save_button = rsx! {
        button {
            id: save_id,
            class: save_class,
            r#type: "button",
            disabled: saving || confirming,
            onclick: move |_| wizard.save(false),
            {save_label}
        }
    };
    let redo_button = rsx! {
        button {
            id: redo_id,
            class: redo_class,
            r#type: "button",
            disabled: saving,
            onclick: move |_| wizard.begin_position(),
            "Redo"
        }
    };
    if poor {
        rsx! { {save_button} {redo_button} }
    } else {
        rsx! { {redo_button} {save_button} }
    }
}

/// The monitor's name, for "Will be saved as".
fn selected_monitor_name(wizard: Wizard) -> String {
    wizard
        .monitors
        .monitors()
        .get(wizard.monitors.selected().unwrap_or(usize::MAX))
        .map_or_else(
            || "this screen".into(),
            |monitor| monitor.display_name().to_string(),
        )
}

#[component]
pub fn ReviewStep(wizard: Wizard) -> Element {
    // Follow the selected size's verdict: Enter saves a clearly good result or
    // redoes a clearly bad one. Amber gets no default: the tech reads first.
    use_effect(move || {
        let use_measured = *wizard.use_measured.read();
        let verdict = wizard
            .finished
            .read()
            .as_ref()
            .map(|f| assess(&f.outcome(use_measured).measures).verdict);
        if matches!(verdict, Some(Verdict::Good | Verdict::Poor)) {
            focus_primary();
        }
    });
    let Some(finished) = wizard.finished.read().clone() else {
        return rsx! {};
    };
    let use_measured = *wizard.use_measured.read();
    let outcome = finished.outcome(use_measured);
    let quality = assess(&outcome.measures);
    let disagreement = outcome.size_check.filter(SizeCheck::disagrees);
    let can_use_measured = finished.measured.is_some();
    let source = source_label(outcome.size_source, finished.size_origin);
    let (icon, title) = headline(quality.verdict);
    let poor = quality.verdict == Verdict::Poor;
    let screen = screen_label(wizard.screen_id());
    let confirming = *wizard.confirm_replace.read();
    let saving = *wizard.saving.read();
    let save_error = wizard.save_error.read().clone();
    let monitor = selected_monitor_name(wizard);
    let camera_sized_and_cropped = outcome.size_source == SizeSource::Measured
        && matches!(outcome.edges, EdgeCheck::Cropped { .. });

    rsx! {
        StepPage {
            title: "Check the result",
            subtitle: "This is what the camera measured.",
            footer: rsx! { ReviewFooter { wizard, poor, saving, confirming } },

            StatusBanner {
                tone: tone(quality.verdict),
                icon,
                title: title,
                detail: quality.advice(),
            }

            if let Some(check) = disagreement {
                DisagreementNotice { wizard, check, size_origin: finished.size_origin, can_use_measured }
            }
            if camera_sized_and_cropped {
                OverscanNotice {}
            }
            if use_measured && can_use_measured {
                UsingMeasuredNotice { wizard }
            }

            ResultOverlay {
                image: finished.image.clone(),
                width: finished.width,
                height: finished.height,
                screen_quad: outcome.screen_quad,
                markers: outcome.markers,
                outline: outline_color(quality.verdict),
            }

            div {
                class: "grid grid-cols-2 gap-3",
                div {
                    class: "rounded-xl bg-white p-4 shadow dark:bg-gray-700",
                    div { class: "text-sm text-gray-500 dark:text-gray-400", "Screen size" }
                    div {
                        class: "text-2xl font-semibold text-gray-900 dark:text-white",
                        "{outcome.diagonal_in:.1} inches"
                    }
                    div {
                        class: "text-sm text-gray-600 dark:text-gray-300",
                        "{outcome.size_m[0] * 100.0:.0} × {outcome.size_m[1] * 100.0:.0} cm"
                    }
                    div { class: "mt-1 text-sm font-medium text-gray-700 dark:text-gray-200", {source} }
                }
                div {
                    class: "rounded-xl bg-white p-4 shadow dark:bg-gray-700",
                    div { class: "text-sm text-gray-500 dark:text-gray-400", "Will be saved as" }
                    div { class: "text-2xl font-semibold text-gray-900 dark:text-white", {screen.as_str()} }
                    div { class: "truncate text-sm text-gray-600 dark:text-gray-300", {monitor} }
                }
            }

            if confirming {
                ReplaceConfirmation { wizard, screen }
            }

            if let Some(error) = save_error {
                SaveError { error }
            }

            Details {
                for check in quality.checks.iter() {
                    Fact { key: "{check.label}", label: check.label.to_string(), value: check.value.clone() }
                }
                Fact {
                    label: "Aligned to gravity",
                    value: if outcome.gravity_aligned { "yes".to_string() } else { "no (camera has no motion sensor data)".to_string() },
                }
                if let Some(circle) = outcome.seed_circle {
                    Fact { label: "Circle through markers 1, 2 and 3", value: circle_text(&circle) }
                }
            }
            SaveAttempt { wizard }
        }
    }
}
