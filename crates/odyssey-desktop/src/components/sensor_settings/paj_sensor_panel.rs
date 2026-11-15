use gpui::component::input::InputState;
use gpui::{
    App, Context, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, Subscription,
    Window, div,
};

use super::field_issue::visible_issue;
use super::gain_table::GAIN_TABLE;
use super::number_field::{
    OptionSelect, bind_select, bind_text, number_row, push_select, push_text, select_row,
};
use super::paj_state::{
    PAJ_AREA_MAX, PAJ_AREA_MIN, PAJ_BRIGHTNESS, PAJ_EXPOSURE, PAJ_FRAME_PERIOD,
    PAJ_FRAME_SUBTRACTION, PAJ_GAIN, PAJ_MAX_OBJECTS, PAJ_NOISE, PAJ_OPERATION_MODE,
    PAJ_RESOLUTION_X, PAJ_RESOLUTION_Y, PajState, exposure_hint, frame_period_hint, paj_validate,
};
use super::settings_form::SettingsForm;
use super::settings_panel::SensorSettings;
use crate::theme::Palette;

/// Which of the form's two PAJ states a panel edits.
pub(super) type PajSlot = fn(&mut SettingsForm) -> &mut PajState;

/// The widgets of one PAJ sensor (near or wide field).
pub(super) struct PajPanel {
    label: &'static str,
    brightness: Entity<InputState>,
    noise: Entity<InputState>,
    area_min: Entity<InputState>,
    area_max: Entity<InputState>,
    max_objects: Entity<InputState>,
    operation_mode: OptionSelect,
    exposure: Entity<InputState>,
    frame_period: Entity<InputState>,
    frame_subtraction: OptionSelect,
    gain: OptionSelect,
    resolution_x: Entity<InputState>,
    resolution_y: Entity<InputState>,
}

impl PajPanel {
    pub(super) fn new(
        label: &'static str,
        slot: PajSlot,
        window: &mut Window,
        cx: &mut Context<SensorSettings>,
        subscriptions: &mut Vec<Subscription>,
    ) -> Self {
        let options = |names: &[&'static str]| -> Vec<SharedString> {
            names.iter().map(|&name| SharedString::from(name)).collect()
        };
        let gain_options = GAIN_TABLE
            .iter()
            .map(|entry| SharedString::from(entry.label))
            .collect();
        Self {
            label,
            brightness: bind_text(window, cx, subscriptions, move |f| {
                &mut slot(f).brightness_threshold
            }),
            noise: bind_text(window, cx, subscriptions, move |f| {
                &mut slot(f).noise_threshold
            }),
            area_min: bind_text(window, cx, subscriptions, move |f| {
                &mut slot(f).area_threshold_min
            }),
            area_max: bind_text(window, cx, subscriptions, move |f| {
                &mut slot(f).area_threshold_max
            }),
            max_objects: bind_text(window, cx, subscriptions, move |f| {
                &mut slot(f).max_object_cnt
            }),
            operation_mode: bind_select(
                window,
                cx,
                subscriptions,
                options(&["Normal", "Tracking"]),
                move |f| &mut slot(f).operation_mode,
            ),
            exposure: bind_text(window, cx, subscriptions, move |f| {
                &mut slot(f).exposure_time
            }),
            frame_period: bind_text(window, cx, subscriptions, move |f| {
                &mut slot(f).frame_period
            }),
            frame_subtraction: bind_select(
                window,
                cx,
                subscriptions,
                options(&["Off", "On"]),
                move |f| &mut slot(f).frame_subtraction,
            ),
            gain: bind_select(window, cx, subscriptions, gain_options, move |f| {
                &mut slot(f).gain_index
            }),
            resolution_x: bind_text(window, cx, subscriptions, move |f| {
                &mut slot(f).resolution_x
            }),
            resolution_y: bind_text(window, cx, subscriptions, move |f| {
                &mut slot(f).resolution_y
            }),
        }
    }

    /// Shows `state` in the widgets.
    pub(super) fn push(&self, state: &PajState, window: &mut Window, cx: &mut App) {
        push_text(&self.brightness, &state.brightness_threshold, window, cx);
        push_text(&self.noise, &state.noise_threshold, window, cx);
        push_text(&self.area_min, &state.area_threshold_min, window, cx);
        push_text(&self.area_max, &state.area_threshold_max, window, cx);
        push_text(&self.max_objects, &state.max_object_cnt, window, cx);
        push_select(&self.operation_mode, &state.operation_mode, window, cx);
        push_text(&self.exposure, &state.exposure_time, window, cx);
        push_text(&self.frame_period, &state.frame_period, window, cx);
        push_select(
            &self.frame_subtraction,
            &state.frame_subtraction,
            window,
            cx,
        );
        push_select(&self.gain, &state.gain_index, window, cx);
        push_text(&self.resolution_x, &state.resolution_x, window, cx);
        push_text(&self.resolution_y, &state.resolution_y, window, cx);
    }

    pub(super) fn render(
        &self,
        state: &PajState,
        show_issues: bool,
        palette: &Palette,
    ) -> impl IntoElement {
        let issues = paj_validate(state).err().unwrap_or_default();
        let issue = |field: &str| visible_issue(&issues, field, show_issues);

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(palette.fg_strong)
                    .child(self.label),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(number_row(
                        PAJ_BRIGHTNESS,
                        &self.brightness,
                        None,
                        issue(PAJ_BRIGHTNESS),
                        palette,
                    ))
                    .child(number_row(
                        PAJ_NOISE,
                        &self.noise,
                        None,
                        issue(PAJ_NOISE),
                        palette,
                    ))
                    .child(number_row(
                        PAJ_AREA_MIN,
                        &self.area_min,
                        Some("px".into()),
                        issue(PAJ_AREA_MIN),
                        palette,
                    ))
                    .child(number_row(
                        PAJ_AREA_MAX,
                        &self.area_max,
                        Some("px".into()),
                        issue(PAJ_AREA_MAX),
                        palette,
                    ))
                    .child(number_row(
                        PAJ_MAX_OBJECTS,
                        &self.max_objects,
                        None,
                        issue(PAJ_MAX_OBJECTS),
                        palette,
                    ))
                    .child(select_row(
                        PAJ_OPERATION_MODE,
                        &self.operation_mode,
                        issue(PAJ_OPERATION_MODE),
                        palette,
                    ))
                    .child(number_row(
                        PAJ_EXPOSURE,
                        &self.exposure,
                        Some(exposure_hint(&state.exposure_time)),
                        issue(PAJ_EXPOSURE),
                        palette,
                    ))
                    .child(number_row(
                        PAJ_FRAME_PERIOD,
                        &self.frame_period,
                        Some(frame_period_hint(&state.frame_period)),
                        issue(PAJ_FRAME_PERIOD),
                        palette,
                    ))
                    .child(select_row(
                        PAJ_FRAME_SUBTRACTION,
                        &self.frame_subtraction,
                        issue(PAJ_FRAME_SUBTRACTION),
                        palette,
                    ))
                    .child(select_row(PAJ_GAIN, &self.gain, issue(PAJ_GAIN), palette))
                    .child(number_row(
                        PAJ_RESOLUTION_X,
                        &self.resolution_x,
                        None,
                        issue(PAJ_RESOLUTION_X),
                        palette,
                    ))
                    .child(number_row(
                        PAJ_RESOLUTION_Y,
                        &self.resolution_y,
                        None,
                        issue(PAJ_RESOLUTION_Y),
                        palette,
                    )),
            )
    }
}
