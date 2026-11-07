//! Step 5: the result. The camera picture with the fitted screen outline and
//! marker positions, the measured size, a green/amber/red verdict, and Save.

use gpui::{
    AnyElement, App, Div, Entity, FontWeight, IntoElement as _, ParentElement as _, Styled as _,
    div,
};
use odyssey_autocalib::session::{EdgeCheck, SizeCheck, SizeSource};

use super::notices::{
    disagreement_notice, overscan_notice, replace_confirmation, save_error, using_measured_notice,
};
use super::result_overlay::{outline_color, result_overlay};
use crate::components::ui::{ButtonKind, button, card};
use crate::components::wizard::{Icon, Tone, status_banner};
use crate::theme::Palette;
use crate::views::calibrate::automatic::attempt_button::save_attempt;
use crate::views::calibrate::automatic::hints::circle_text;
use crate::views::calibrate::automatic::kind::source_label;
use crate::views::calibrate::automatic::quality::{Verdict, assess};
use crate::views::calibrate::automatic::screens::screen_label;
use crate::views::calibrate::automatic::ui::{act, details, fact, step_page};
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
fn review_footer(
    p: Palette,
    wizard: &Entity<Wizard>,
    poor: bool,
    saving: bool,
    confirming: bool,
) -> Vec<AnyElement> {
    let (save_kind, redo_kind) = if poor {
        (ButtonKind::Secondary, ButtonKind::Primary)
    } else {
        (ButtonKind::Primary, ButtonKind::Secondary)
    };
    let save_label = if saving {
        "Saving…"
    } else if poor {
        "Save anyway"
    } else {
        "Save"
    };
    let save_button = button("save", save_label, save_kind, p)
        .disabled(saving || confirming)
        .on_click(act(wizard, |w, cx| w.save(false, cx)))
        .into_any_element();
    let redo_button = button("redo", "Redo", redo_kind, p)
        .disabled(saving)
        .on_click(act(wizard, |w, cx| w.begin_position(cx)))
        .into_any_element();
    if poor {
        vec![save_button, redo_button]
    } else {
        vec![redo_button, save_button]
    }
}

/// The monitor's name, for "Will be saved as".
fn selected_monitor_name(wizard: &Wizard, cx: &App) -> String {
    let monitors = wizard.monitors.read(cx);
    monitors
        .selected()
        .and_then(|index| monitors.monitors().get(index))
        .map_or_else(
            || "this screen".into(),
            |monitor| monitor.display_name().to_string(),
        )
}

fn stat_card(p: Palette) -> Div {
    card(p).p_4()
}

pub(in crate::views::calibrate::automatic) fn review_step(
    p: Palette,
    wizard: &Entity<Wizard>,
    details_open: &Entity<bool>,
    difference_open: &Entity<bool>,
    cx: &App,
) -> Div {
    let state = wizard.read(cx);
    let Some(finished) = state.finished.clone() else {
        return div();
    };
    let use_measured = state.use_measured;
    let outcome = finished.outcome(use_measured);
    let quality = assess(&outcome.measures);
    let disagreement = outcome.size_check.filter(SizeCheck::disagrees);
    let can_use_measured = finished.measured.is_some();
    let source = source_label(outcome.size_source, finished.size_origin);
    let (glyph, title) = headline(quality.verdict);
    let poor = quality.verdict == Verdict::Poor;
    let screen = screen_label(state.screen_id());
    let confirming = state.confirm_replace;
    let saving = state.saving;
    let monitor = selected_monitor_name(state, cx);
    let camera_sized_and_cropped = outcome.size_source == SizeSource::Measured
        && matches!(outcome.edges, EdgeCheck::Cropped { .. });

    let mut content = vec![
        status_banner(p, tone(quality.verdict), glyph, title, quality.advice()).into_any_element(),
    ];
    if let Some(check) = disagreement {
        content.push(disagreement_notice(
            p,
            wizard,
            &check,
            finished.size_origin,
            can_use_measured,
            difference_open,
            cx,
        ));
    }
    if camera_sized_and_cropped {
        content.push(overscan_notice(p));
    }
    if use_measured && can_use_measured {
        content.push(using_measured_notice(p, wizard, cx));
    }

    content.push(
        result_overlay(
            p,
            finished.image.clone(),
            finished.width,
            finished.height,
            outcome.screen_quad,
            outcome.markers,
            outline_color(p, quality.verdict),
        )
        .into_any_element(),
    );

    content.push(
        div()
            .grid()
            .grid_cols(2)
            .gap_3()
            .child(
                stat_card(p)
                    .child(div().text_sm().text_color(p.fg_muted).child("Screen size"))
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.fg_strong)
                            .child(format!("{:.1} inches", outcome.diagonal_in)),
                    )
                    .child(div().text_sm().text_color(p.fg_muted).child(format!(
                        "{:.0} × {:.0} cm",
                        outcome.size_m[0] * 100.0,
                        outcome.size_m[1] * 100.0
                    )))
                    .child(
                        div()
                            .mt_1()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.fg)
                            .child(source),
                    ),
            )
            .child(
                stat_card(p)
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.fg_muted)
                            .child("Will be saved as"),
                    )
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.fg_strong)
                            .child(screen.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(p.fg_muted)
                            .child(monitor),
                    ),
            )
            .into_any_element(),
    );

    if confirming {
        content.push(replace_confirmation(p, wizard, &screen, cx));
    }
    if let Some(error) = state.save_error.as_deref() {
        content.push(save_error(p, error).into_any_element());
    }

    let mut facts: Vec<AnyElement> = quality
        .checks
        .iter()
        .map(|check| fact(p, check.label, check.value.clone()))
        .collect();
    facts.push(fact(
        p,
        "Aligned to gravity",
        if outcome.gravity_aligned {
            "yes"
        } else {
            "no (camera has no motion sensor data)"
        },
    ));
    if let Some(circle) = outcome.seed_circle {
        facts.push(fact(
            p,
            "Circle through markers 1, 2 and 3",
            circle_text(&circle),
        ));
    }
    content
        .push(details("review-details", p, "Details", facts, details_open, cx).into_any_element());
    content.push(save_attempt(p, wizard, state).into_any_element());

    step_page(
        p,
        "Check the result",
        Some("This is what the camera measured.".into()),
        content,
        review_footer(p, wizard, poor, saving, confirming),
    )
}
