//! Shown after a successful save.

use gpui::{App, Div, Entity, FontWeight, IntoElement as _, ParentElement as _, Styled as _, div};

use crate::components::ui::{AlertKind, ButtonKind, alert, button, card};
use crate::components::wizard::{Icon, icon};
use crate::theme::Palette;
use crate::views::calibrate::automatic::screens::screen_label;
use crate::views::calibrate::automatic::ui::{act, details, fact, step_page};
use crate::views::calibrate::automatic::wizard::Wizard;

pub(in crate::views::calibrate::automatic) fn saved_step(
    p: Palette,
    wizard: &Entity<Wizard>,
    details_open: &Entity<bool>,
    cx: &App,
) -> Div {
    let Some(saved) = wizard.read(cx).saved_to.clone() else {
        return div();
    };
    step_page(
        p,
        "Calibration saved",
        Some(format!("{} is ready to use.", screen_label(saved.screen_id)).into()),
        vec![
            alert(
                AlertKind::Success,
                p,
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .px_6()
                    .py_8()
                    .text_center()
                    .child(icon(Icon::Check, 64.0, p.success_ink))
                    .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child("All done"))
                    .child(div().text_base().child(
                        "The new calibration is active as soon as the hub picks it up, which takes a second or two.",
                    )),
            )
            .into_any_element(),
            card(p)
                .flex_row()
                .items_center()
                .justify_between()
                .gap_4()
                .px_5()
                .py_4()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .text_base()
                        .text_color(p.fg)
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.fg_strong)
                                .child("Recommended:"),
                        )
                        .child("re-zero your devices so aiming matches the new calibration."),
                )
                .child(
                    button("go-to-zero", "Zero tracker", ButtonKind::Secondary, p)
                        .on_click(act(wizard, Wizard::go_home)),
                )
                .into_any_element(),
            details(
                "saved-details",
                p,
                "Details",
                vec![fact(p, "Saved to", saved.path)],
                details_open,
                cx,
            )
            .into_any_element(),
        ],
        vec![
            button("another", "Calibrate another screen", ButtonKind::Secondary, p)
                .on_click(act(wizard, Wizard::calibrate_another))
                .into_any_element(),
            button("done", "Done", ButtonKind::Primary, p)
                .on_click(act(wizard, Wizard::exit))
                .into_any_element(),
        ],
    )
}
