//! Step 5: the result. The camera picture with the fitted screen outline and
//! marker positions, the measured size, a green/amber/red verdict, and Save.
//! Then the success screen with what to do next.

use dioxus::prelude::*;
use odyssey_autocalib::session::{EdgeCheck, SizeSource};

use super::attempt_button::SaveAttempt;
use super::hints::circle_text;
use super::kind::{SizeOrigin, source_label};
use super::quality::{Verdict, assess, disagreement_message};
use super::screens::screen_label;
use super::ui::{
    Details, Fact, PRIMARY_BUTTON, PRIMARY_ID, SECONDARY_BUTTON, StepPage, focus_primary,
};
use super::wizard::Wizard;
use crate::components::wizard::{Icon, IconView, OverlayImage, StatusBanner, Tone};

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

#[component]
pub fn ReviewStep(wizard: Wizard) -> Element {
    // Enter does the obvious thing when the result is clearly good (save) or
    // clearly bad (redo). An amber result gets no default: the tech reads first.
    use_effect(move || {
        let verdict = wizard
            .finished
            .read()
            .as_ref()
            .map(|f| assess(&f.outcome(false).measures).verdict);
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
    let disagreement = outcome.size_check.filter(|c| c.disagrees());
    let can_use_measured = finished.measured.is_some();
    let source = source_label(outcome.size_source, finished.size_origin);
    let (icon, title) = headline(quality.verdict);
    let poor = quality.verdict == Verdict::Poor;
    let screen = screen_label(wizard.screen_id());
    let confirming = *wizard.confirm_replace.read();
    let saving = *wizard.saving.read();
    let save_error = wizard.save_error.read().clone();
    let monitor = wizard
        .monitors
        .monitors()
        .get(wizard.monitors.selected().unwrap_or(usize::MAX))
        .map(|m| m.display_name().to_string())
        .unwrap_or_else(|| "this screen".into());

    let quad = outcome
        .screen_quad
        .iter()
        .map(|p| format!("{:.1},{:.1}", p[0], p[1]))
        .collect::<Vec<_>>()
        .join(" ");
    let unit = f64::from(finished.width) / 640.0;
    let outline = quality_color(quality.verdict);

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

    rsx! {
        StepPage {
            title: "Check the result",
            subtitle: "This is what the camera measured.",
            footer: rsx! {
                // The recommended action sits on the right, where the eye ends.
                if poor {
                    button {
                        class: save_class,
                        r#type: "button",
                        disabled: saving || confirming,
                        onclick: move |_| wizard.save(false),
                        "{save_label}"
                    }
                    button {
                        id: PRIMARY_ID,
                        class: redo_class,
                        r#type: "button",
                        onclick: move |_| wizard.begin_position(),
                        "Redo"
                    }
                } else {
                    button {
                        class: redo_class,
                        r#type: "button",
                        onclick: move |_| wizard.begin_position(),
                        "Redo"
                    }
                    button {
                        id: PRIMARY_ID,
                        class: save_class,
                        r#type: "button",
                        disabled: saving || confirming,
                        onclick: move |_| wizard.save(false),
                        "{save_label}"
                    }
                }
            },

            StatusBanner {
                tone: tone(quality.verdict),
                icon,
                title: title,
                detail: quality.advice(),
            }

            if let Some(check) = disagreement {
                div {
                    role: "alert",
                    class: "flex flex-col gap-3 rounded-xl border-2 border-amber-300 bg-amber-50 p-4 text-amber-900 dark:border-amber-700 dark:bg-amber-950/60 dark:text-amber-100",
                    div {
                        class: "flex items-start gap-3",
                        IconView { icon: Icon::Warning, class: "mt-0.5 size-6 shrink-0 text-amber-600 dark:text-amber-400".to_string() }
                        div {
                            class: "flex flex-col gap-1",
                            div { class: "text-lg font-semibold", "The camera and the screen's size don't agree" }
                            div { class: "text-base", "{disagreement_message(&check, finished.size_origin)}" }
                        }
                    }
                    if can_use_measured {
                        div {
                            button {
                                class: SECONDARY_BUTTON,
                                r#type: "button",
                                onclick: move |_| *wizard.use_measured.write() = true,
                                "Use the camera's measurement instead"
                            }
                        }
                        details {
                            class: "text-sm",
                            summary { class: "cursor-pointer select-none font-medium", "What's the difference?" }
                            p {
                                class: "mt-2",
                                "A size from the TV or the box is exact if the TV shows its whole picture. The camera's own measurement is usually within about half a percent of the distance, so a few millimeters per meter, which makes it a good second opinion but slightly less exact. If the TV crops its picture, the camera's measurement is the one that matches what you see."
                            }
                        }
                    }
                    div {
                        class: "flex flex-col gap-1",
                        button {
                            class: SECONDARY_BUTTON,
                            r#type: "button",
                            onclick: move |_| wizard.check_by_hand(),
                            if finished.size_origin == Some(SizeOrigin::Reference) {
                                "Measure by hand again"
                            } else {
                                "Settle it with a tape measure or credit card"
                            }
                        }
                        span {
                            class: "text-sm",
                            "Goes back to Setup with a tape measure line on the screen. About a minute, then measure again."
                        }
                    }
                }
            }
            if outcome.size_source == SizeSource::Measured
                && matches!(outcome.edges, EdgeCheck::Cropped { .. })
            {
                div {
                    class: "flex items-start gap-3 rounded-xl bg-sky-50 px-4 py-3 text-base text-sky-900 dark:bg-sky-950/50 dark:text-sky-100",
                    IconView { icon: Icon::Crop, class: "mt-0.5 size-6 shrink-0 text-sky-600 dark:text-sky-300".to_string() }
                    span {
                        "This screen hides the edge of its picture. The size above is the whole picture the computer draws, including the part you can't see, and the calibration is for that picture. Turning off overscan in the TV's picture settings ('Just Scan' or 'PC mode') makes all of it visible, which also lets the TV's own size be used."
                    }
                }
            }
            if use_measured && can_use_measured {
                div {
                    class: "flex flex-wrap items-center justify-between gap-3 rounded-xl bg-sky-50 px-4 py-3 text-base text-sky-900 dark:bg-sky-950/50 dark:text-sky-100",
                    span { "Using the camera's own measurement instead of the declared size." }
                    button {
                        class: SECONDARY_BUTTON,
                        r#type: "button",
                        onclick: move |_| *wizard.use_measured.write() = false,
                        "Go back to the declared size"
                    }
                }
            }

            OverlayImage {
                src: finished.image.clone(),
                width: finished.width,
                height: finished.height,
                alt: "The screen as the camera saw it, with the measured outline",
                polygon {
                    points: "{quad}",
                    fill: "none",
                    stroke: "{outline}",
                    stroke_width: "{3.0 * unit}",
                    stroke_linejoin: "round",
                }
                for (i, marker) in outcome.markers.iter().enumerate() {
                    circle {
                        key: "{i}",
                        cx: "{marker[0]}",
                        cy: "{marker[1]}",
                        r: "{9.0 * unit}",
                        fill: "none",
                        stroke: "#38bdf8",
                        stroke_width: "{2.5 * unit}",
                    }
                    text {
                        x: "{f64::from(marker[0]) + 12.0 * unit}",
                        y: "{f64::from(marker[1]) - 10.0 * unit}",
                        fill: "#38bdf8",
                        font_size: "{16.0 * unit}",
                        font_weight: "700",
                        "{i + 1}"
                    }
                }
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
                    div { class: "mt-1 text-sm font-medium text-gray-700 dark:text-gray-200", "{source}" }
                }
                div {
                    class: "rounded-xl bg-white p-4 shadow dark:bg-gray-700",
                    div { class: "text-sm text-gray-500 dark:text-gray-400", "Will be saved as" }
                    div { class: "text-2xl font-semibold text-gray-900 dark:text-white", "{screen}" }
                    div { class: "truncate text-sm text-gray-600 dark:text-gray-300", "{monitor}" }
                }
            }

            if confirming {
                div {
                    role: "alertdialog",
                    aria_label: "Replace the existing calibration?",
                    class: "flex flex-col gap-3 rounded-xl border-2 border-amber-300 bg-amber-50 p-4 text-amber-900 dark:border-amber-700 dark:bg-amber-950/60 dark:text-amber-100",
                    div { class: "text-lg font-semibold", "{screen} already has a calibration" }
                    div { class: "text-base", "Saving replaces it. The old calibration can't be brought back." }
                    div {
                        class: "flex gap-3",
                        button {
                            class: PRIMARY_BUTTON,
                            r#type: "button",
                            onclick: move |_| wizard.save(true),
                            "Replace it"
                        }
                        button {
                            class: SECONDARY_BUTTON,
                            r#type: "button",
                            onclick: move |_| *wizard.confirm_replace.write() = false,
                            "Keep the old one"
                        }
                    }
                }
            }

            if let Some(error) = save_error {
                div {
                    role: "alert",
                    class: "rounded-xl border-2 border-red-300 bg-red-50 px-4 py-3 text-base text-red-900 dark:border-red-700 dark:bg-red-950/60 dark:text-red-100",
                    "Couldn't save the calibration: {error}"
                }
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

fn quality_color(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Good => "#22c55e",
        Verdict::Fair => "#f59e0b",
        Verdict::Poor => "#ef4444",
    }
}

/// Shown after a successful save.
#[component]
pub fn SavedStep(wizard: Wizard) -> Element {
    let Some(saved) = wizard.saved_to.read().clone() else {
        return rsx! {};
    };
    rsx! {
        StepPage {
            title: "Calibration saved",
            subtitle: "{screen_label(saved.screen_id)} is ready to use.",
            footer: rsx! {
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.calibrate_another(),
                    "Calibrate another screen"
                }
                button {
                    id: PRIMARY_ID,
                    class: PRIMARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.exit(),
                    "Done"
                }
            },

            div {
                class: "flex flex-col items-center gap-3 rounded-xl border-2 border-emerald-300 bg-emerald-50 px-6 py-8 text-center text-emerald-900 dark:border-emerald-700 dark:bg-emerald-950/60 dark:text-emerald-100",
                IconView { icon: Icon::Check, class: "size-16 text-emerald-600 dark:text-emerald-400".to_string() }
                div { class: "text-2xl font-semibold", "All done" }
                div { class: "text-base", "The new calibration is active as soon as the hub picks it up, which takes a second or two." }
            }

            section {
                class: "flex items-center justify-between gap-4 rounded-xl bg-white px-5 py-4 shadow dark:bg-gray-700",
                p {
                    class: "text-base text-gray-700 dark:text-gray-200",
                    span { class: "font-semibold text-gray-900 dark:text-white", "Recommended: " }
                    "re-zero your devices so aiming matches the new calibration."
                }
                button {
                    class: SECONDARY_BUTTON,
                    r#type: "button",
                    onclick: move |_| wizard.go_home(),
                    "Go to Zero"
                }
            }

            Details {
                Fact { label: "Saved to", value: saved.path.clone() }
            }
        }
    }
}
