//! The boxes the review shows above and below the camera picture.

use gpui::{
    AnyElement, App, Div, Entity, FontWeight, IntoElement as _, ParentElement as _, Styled as _,
    div,
};
use odyssey_autocalib::session::SizeCheck;

use crate::components::ui::{AlertKind, ButtonKind, alert, button};
use crate::components::wizard::{Icon, icon};
use crate::theme::Palette;
use crate::views::calibrate::automatic::kind::SizeOrigin;
use crate::views::calibrate::automatic::quality::disagreement_message;
use crate::views::calibrate::automatic::ui::{act, info_note, plain_details};
use crate::views::calibrate::automatic::wizard::Wizard;

/// The camera and the declared size disagree: what that means and what to do.
pub(super) fn disagreement_notice(
    p: &Palette,
    wizard: &Entity<Wizard>,
    check: &SizeCheck,
    size_origin: Option<SizeOrigin>,
    can_use_measured: bool,
    difference_open: &Entity<bool>,
    cx: &App,
) -> AnyElement {
    let saving = wizard.read(cx).saving;
    let mut body = div().flex().flex_col().gap_3().child(
        div()
            .flex()
            .items_start()
            .gap_3()
            .child(
                div()
                    .mt_0p5()
                    .flex_shrink_0()
                    .child(icon(Icon::Warning, 24.0, p.warning_ink)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("The camera and the screen's size don't agree"),
                    )
                    .child(
                        div()
                            .text_base()
                            .child(disagreement_message(check, size_origin)),
                    ),
            ),
    );
    if can_use_measured {
        body = body
            .child(
                div().child(
                    button(
                        "use-measured",
                        "Use the camera's measurement instead",
                        ButtonKind::Secondary,
                        p,
                    )
                    .disabled(saving)
                    .on_click(act(wizard, |w, cx| {
                        w.use_measured = true;
                        cx.notify();
                    })),
                ),
            )
            .child(plain_details(
                "review-difference",
                p,
                "What's the difference?",
                vec![div().child("A size from the TV or the box is exact if the TV shows its whole picture. The camera's own measurement is usually within about half a percent of the distance, so a few millimeters per meter, which makes it a good second opinion but slightly less exact. If the TV crops its picture, the camera's measurement is the one that matches what you see.").into_any_element()],
                difference_open,
                cx,
            ));
    }
    body = body.child(
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                button(
                    "check-by-hand",
                    if size_origin == Some(SizeOrigin::Reference) {
                        "Measure by hand again"
                    } else {
                        "Settle it with a tape measure or credit card"
                    },
                    ButtonKind::Secondary,
                p,
                )
                .disabled(saving)
                .on_click(act(wizard, Wizard::check_by_hand)),
            )
            .child(div().text_sm().child(
                "Goes back to Setup with a tape measure line on the screen. About a minute, then measure again.",
            )),
    );
    alert(AlertKind::Warning, p, body).into_any_element()
}

/// The camera sized a screen that hides the edge of its picture.
pub(super) fn overscan_notice(p: &Palette) -> AnyElement {
    info_note(
        p,
        div().child("This screen hides the edge of its picture. The size above is the whole picture the computer draws, including the part you can't see, and the calibration is for that picture. Turning off overscan in the TV's picture settings ('Just Scan' or 'PC mode') makes all of it visible, which also lets the TV's own size be used."),
    )
}

/// The review is showing the camera's own size instead of the declared one.
pub(super) fn using_measured_notice(p: &Palette, wizard: &Entity<Wizard>, cx: &App) -> AnyElement {
    let saving = wizard.read(cx).saving;
    alert(
        AlertKind::Info,
        p,
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .text_base()
            .child("Using the camera's own measurement instead of the declared size.")
            .child(
                button(
                    "use-declared",
                    "Go back to the declared size",
                    ButtonKind::Secondary,
                    p,
                )
                .disabled(saving)
                .on_click(act(wizard, |w, cx| {
                    w.use_measured = false;
                    cx.notify();
                })),
            ),
    )
    .into_any_element()
}

/// Saving would overwrite a calibration that is already stored for the screen.
pub(super) fn replace_confirmation(
    p: &Palette,
    wizard: &Entity<Wizard>,
    screen: &str,
    cx: &App,
) -> AnyElement {
    let saving = wizard.read(cx).saving;
    alert(
        AlertKind::Warning,
        p,
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{screen} already has a calibration")),
            )
            .child(
                div()
                    .text_base()
                    .child("Saving replaces it. The old calibration can't be brought back."),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        button("replace", "Replace it", ButtonKind::Primary, p)
                            .disabled(saving)
                            .on_click(act(wizard, |w, cx| w.save(true, cx))),
                    )
                    .child(
                        button("keep-old", "Keep the old one", ButtonKind::Secondary, p)
                            .disabled(saving)
                            .on_click(act(wizard, |w, cx| {
                                w.confirm_replace = false;
                                cx.notify();
                            })),
                    ),
            ),
    )
    .into_any_element()
}

pub(super) fn save_error(p: &Palette, error: &str) -> Div {
    div().child(alert(
        AlertKind::Danger,
        p,
        div()
            .text_base()
            .child(format!("Couldn't save the calibration: {error}")),
    ))
}
