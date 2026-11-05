use gpui::component::input::InputState;
use gpui::{
    App, Context, Entity, IntoElement, ParentElement as _, Styled as _, Subscription, Window, div,
};

use super::field_issue::visible_issue;
use super::number_field::{bind_text, number_row, push_text};
use super::pag_state::{
    PAG_AREA_MAX, PAG_AREA_MIN, PAG_EXPOSURE, PAG_FPS, PAG_GAIN, PAG_LIGHT, PagState,
    exposure_hint, pag_validate,
};
use super::settings_panel::SensorSettings;
use crate::theme::Palette;

/// The widgets of the PAG sensor.
pub(super) struct PagPanel {
    fps: Entity<InputState>,
    exposure: Entity<InputState>,
    gain: Entity<InputState>,
    light: Entity<InputState>,
    area_min: Entity<InputState>,
    area_max: Entity<InputState>,
}

impl PagPanel {
    pub(super) fn new(
        window: &mut Window,
        cx: &mut Context<SensorSettings>,
        subscriptions: &mut Vec<Subscription>,
    ) -> Self {
        Self {
            fps: bind_text(window, cx, subscriptions, |f| &mut f.pag.fps),
            exposure: bind_text(window, cx, subscriptions, |f| &mut f.pag.exposure_us),
            gain: bind_text(window, cx, subscriptions, |f| &mut f.pag.gain),
            light: bind_text(window, cx, subscriptions, |f| &mut f.pag.light_threshold),
            area_min: bind_text(window, cx, subscriptions, |f| &mut f.pag.area_lower),
            area_max: bind_text(window, cx, subscriptions, |f| &mut f.pag.area_upper),
        }
    }

    /// Shows `state` in the widgets.
    pub(super) fn push(&self, state: &PagState, window: &mut Window, cx: &mut App) {
        push_text(&self.fps, &state.fps, window, cx);
        push_text(&self.exposure, &state.exposure_us, window, cx);
        push_text(&self.gain, &state.gain, window, cx);
        push_text(&self.light, &state.light_threshold, window, cx);
        push_text(&self.area_min, &state.area_lower, window, cx);
        push_text(&self.area_max, &state.area_upper, window, cx);
    }

    pub(super) fn render(
        &self,
        state: &PagState,
        show_issues: bool,
        palette: Palette,
    ) -> impl IntoElement {
        let issues = pag_validate(state).err().unwrap_or_default();
        let issue = |field: &str| visible_issue(&issues, field, show_issues);
        let exposure_hint = exposure_hint(&state.exposure_us);

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(palette.fg_strong)
                    .child("Sensor"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(number_row(
                        PAG_FPS,
                        &self.fps,
                        Some("fps".into()),
                        issue(PAG_FPS),
                        palette,
                    ))
                    .child(number_row(
                        PAG_EXPOSURE,
                        &self.exposure,
                        Some(format!("µs · {exposure_hint}")),
                        issue(PAG_EXPOSURE),
                        palette,
                    ))
                    .child(number_row(
                        PAG_GAIN,
                        &self.gain,
                        None,
                        issue(PAG_GAIN),
                        palette,
                    ))
                    .child(number_row(
                        PAG_LIGHT,
                        &self.light,
                        None,
                        issue(PAG_LIGHT),
                        palette,
                    ))
                    .child(number_row(
                        PAG_AREA_MIN,
                        &self.area_min,
                        Some("px".into()),
                        issue(PAG_AREA_MIN),
                        palette,
                    ))
                    .child(number_row(
                        PAG_AREA_MAX,
                        &self.area_max,
                        Some("px".into()),
                        issue(PAG_AREA_MAX),
                        palette,
                    )),
            )
    }
}
