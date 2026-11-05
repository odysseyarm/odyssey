use odyssey_hub_client::client::Client;
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::InputState;
use gpui::component::{Disableable as _, Sizable as _};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AsyncWindowContext, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    WeakEntity, Window, div, px,
};
use odyssey_hub_common::device::{Device, DeviceCapabilities};

use super::field_issue::{FieldIssue, visible_issue};
use super::general_config::{IMPACT_THRESHOLD_FIELD, SHOT_DELAY_FIELD, SUPPRESS_FIELD};
use super::number_field::{bind_text, number_row, push_text};
use super::op_status::{rpc_failure, status_line};
use super::pag_sensor_panel::PagPanel;
use super::paj_sensor_panel::PajPanel;
use super::product_ids::SensorFamily;
use super::settings_form::{
    LoadTarget, SettingsForm, read_from_device, write_to_device,
};
use crate::hub::HubHandle;
use crate::runtime;
use crate::theme::{Palette, RADIUS_SM, palette};

#[derive(Clone, Copy)]
enum SettingsOperation {
    Load,
    LoadSensors,
    Write(bool),
    RestoreDefaults,
}

/// The "Device Settings" disclosure of one device row.
pub(crate) struct SensorSettings {
    pub(super) form: SettingsForm,
    target: LoadTarget,
    /// Distinguishes this panel's element ids from other rows'.
    key: String,
    is_open: bool,
    details_open: bool,
    /// Serializes all form operations so a queued reload cannot race a save or
    /// overwrite defaults that the user filled after requesting that reload.
    queue: UnboundedSender<SettingsOperation>,
    impact_threshold: Entity<InputState>,
    suppress_ms: Entity<InputState>,
    shot_delay: Entity<InputState>,
    paj_near_field: PajPanel,
    paj_wide_field: PajPanel,
    pag: PagPanel,
    _subscriptions: Vec<Subscription>,
    _queue_task: Task<()>,
}

fn target_for(device: Device) -> LoadTarget {
    // General settings can use the control plane; register access needs a live
    // events connection, whether it arrives over USB or Bluetooth.
    let can_tune_sensors =
        device.capabilities.contains(DeviceCapabilities::EVENTS) && device.events_connected;
    LoadTarget {
        device,
        can_tune_sensors,
    }
}

impl SensorSettings {
    pub(crate) fn new(device: Device, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let (queue, mut operations) = unbounded();
        let queue_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(operation) = operations.next().await {
                if run_operation(&this, operation, cx).await.is_none() {
                    break;
                }
            }
        });
        let this = Self {
            form: SettingsForm::default(),
            key: hex::encode(device.uuid),
            target: target_for(device),
            is_open: false,
            details_open: false,
            queue,
            impact_threshold: bind_text(window, cx, &mut subscriptions, |f| {
                &mut f.impact_threshold
            }),
            suppress_ms: bind_text(window, cx, &mut subscriptions, |f| &mut f.suppress_ms),
            shot_delay: bind_text(window, cx, &mut subscriptions, |f| &mut f.shot_delay),
            paj_near_field: PajPanel::new(
                "Near Field",
                |f| &mut f.paj_near_field,
                window,
                cx,
                &mut subscriptions,
            ),
            paj_wide_field: PajPanel::new(
                "Wide Field",
                |f| &mut f.paj_wide_field,
                window,
                cx,
                &mut subscriptions,
            ),
            pag: PagPanel::new(window, cx, &mut subscriptions),
            _subscriptions: subscriptions,
            _queue_task: queue_task,
        };
        // Dropdowns show the form's defaults from the first paint.
        this.push_all(window, cx);
        this
    }

    /// Follows the device as the hub reports it. Keeps the load target
    /// current, and fills the sensor panels as soon as they appear: they render
    /// empty until something loads them, and a form that was loaded before the
    /// events link came up never asked for them. Only the newly available
    /// sensors are loaded, so unsaved general edits survive.
    pub(crate) fn set_device(&mut self, device: Device, cx: &mut Context<Self>) {
        let target = target_for(device);
        if target == self.target {
            return;
        }
        let panels_appeared = target.can_tune_sensors
            && (!self.target.can_tune_sensors
                || self.target.device.product_id != target.device.product_id);
        self.target = target;
        if panels_appeared && self.is_open {
            self.send(SettingsOperation::LoadSensors);
        }
        cx.notify();
    }

    fn send(&self, operation: SettingsOperation) {
        // The queue task lives as long as `self`, so a failed send only means
        // the panel is being torn down.
        let _ = self.queue.unbounded_send(operation);
    }

    /// Shows the form's text in every widget after the form changed underneath
    /// them.
    fn push_all(&self, window: &mut Window, cx: &mut Context<Self>) {
        push_text(&self.impact_threshold, &self.form.impact_threshold, window, cx);
        push_text(&self.suppress_ms, &self.form.suppress_ms, window, cx);
        push_text(&self.shot_delay, &self.form.shot_delay, window, cx);
        self.paj_near_field.push(&self.form.paj_near_field, window, cx);
        self.paj_wide_field.push(&self.form.paj_wide_field, window, cx);
        self.pag.push(&self.form.pag, window, cx);
    }

    fn hub_client(cx: &Context<Self>) -> Option<Client> {
        cx.global::<HubHandle>().0.read(cx).client()
    }

    fn render_actions(&self, busy: bool, has_invalid: bool, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let key = &self.key;
        let button = |name: &str, label: &'static str, tip: &'static str| {
            Button::new(SharedString::from(format!("sensor-{name}-{key}")))
                .label(label)
                .tooltip(tip)
                .small()
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .flex_wrap()
            .child(
                button(
                    "save",
                    "Save to device",
                    "Write these values and keep them across power cycles",
                )
                .primary()
                .disabled(busy || has_invalid)
                .on_click(cx.listener(|this, _, _, _| {
                    this.send(SettingsOperation::Write(true));
                })),
            )
            .child(
                button(
                    "try",
                    "Try without saving",
                    "Write these values for this session only",
                )
                .outline()
                .disabled(busy || has_invalid)
                .on_click(cx.listener(|this, _, _, _| {
                    this.send(SettingsOperation::Write(false));
                })),
            )
            .child(
                button(
                    "reload",
                    "Reload",
                    "Discard edits and re-read the device",
                )
                .outline()
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, _| this.send(SettingsOperation::Load))),
            )
            .child(
                button(
                    "restore",
                    "Restore defaults",
                    "Fill the form with factory values; nothing is written yet",
                )
                .outline()
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, _| {
                    this.send(SettingsOperation::RestoreDefaults);
                })),
            )
            .child(status_line(
                &self.form.status,
                format!("sensor-details-{key}"),
                self.details_open,
                cx.listener(|this, _, _, cx| {
                    this.details_open = !this.details_open;
                    cx.notify();
                }),
                palette,
            ))
    }

    fn render_general(
        &self,
        issues: &[FieldIssue],
        show_issues: bool,
        palette: Palette,
    ) -> impl IntoElement {
        let issue = |field: &str| visible_issue(issues, field, show_issues);
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                section(
                    "Impact detection",
                    palette,
                    [
                        number_row(
                            IMPACT_THRESHOLD_FIELD,
                            &self.impact_threshold,
                            None,
                            issue(IMPACT_THRESHOLD_FIELD),
                            palette,
                        )
                        .into_any_element(),
                        number_row(
                            SUPPRESS_FIELD,
                            &self.suppress_ms,
                            Some("ms".into()),
                            issue(SUPPRESS_FIELD),
                            palette,
                        )
                        .into_any_element(),
                    ],
                ),
            )
            // Shot delay lives on this computer, not on the device.
            .child(
                section(
                    "This computer",
                    palette,
                    [
                        number_row(
                            SHOT_DELAY_FIELD,
                            &self.shot_delay,
                            Some("ms".into()),
                            issue(SHOT_DELAY_FIELD),
                            palette,
                        )
                        .into_any_element(),
                        div()
                            .text_xs()
                            .text_color(palette.fg_muted)
                            .child("Stored on this computer, not on the device.")
                            .into_any_element(),
                    ],
                ),
            )
    }

    /// The per-sensor panels, or a pointer to USB when register access is down.
    fn render_tuning(
        &self,
        sensors: Option<SensorFamily>,
        show_issues: bool,
        palette: Palette,
    ) -> gpui::AnyElement {
        match sensors {
            Some(SensorFamily::Paj) => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    self.paj_near_field
                        .render(&self.form.paj_near_field, show_issues, palette),
                )
                .child(
                    self.paj_wide_field
                        .render(&self.form.paj_wide_field, show_issues, palette),
                )
                .into_any_element(),
            Some(SensorFamily::Pag) => self
                .pag
                .render(&self.form.pag, show_issues, palette)
                .into_any_element(),
            None => div()
                .flex()
                .flex_col()
                .gap_1()
                .border_t_1()
                .border_color(palette.line)
                .pt_2()
                .child(heading("Sensor tuning", palette))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette.fg_muted)
                        .child(
                            "Sensor tuning needs an active sensor data connection. Connect \
                             over USB or Bluetooth, then reload the settings.",
                        ),
                )
                .into_any_element(),
        }
    }
}

fn heading(text: &'static str, palette: Palette) -> impl IntoElement {
    div()
        .text_sm()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(palette.fg_strong)
        .child(text)
}

fn section(
    title: &'static str,
    palette: Palette,
    rows: impl IntoIterator<Item = gpui::AnyElement>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(heading(title, palette))
        .child(div().flex().flex_col().gap_1p5().children(rows))
}

impl Render for SensorSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let device = &self.target.device;
        let has_control = device.capabilities.contains(DeviceCapabilities::CONTROL);
        let has_events = device.capabilities.contains(DeviceCapabilities::EVENTS);
        let family = SensorFamily::for_product(device.product_id);
        if family.is_none() || !(has_control || has_events) {
            return div();
        }
        let palette = palette(cx);
        let tunable_sensors = self.target.tunable_sensors();
        let general_issues = self.form.general_issues();
        let has_invalid = self.form.has_invalid_input(tunable_sensors);
        let busy = self.form.loading;
        // Writes stay blocked on invalid input either way; only the inline
        // markers wait for the first load.
        let show_issues = self.form.hydrated;

        let summary = div()
            .id(SharedString::from(format!("sensor-settings-toggle-{}", self.key)))
            .text_sm()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(palette.fg)
            .cursor_pointer()
            .hover(|style| style.text_color(palette.accent_ink))
            .child(if self.is_open {
                "▾ Device Settings"
            } else {
                "▸ Device Settings"
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.is_open = !this.is_open;
                if this.is_open {
                    this.send(SettingsOperation::Load);
                }
                cx.notify();
            }));

        div()
            .mt_2()
            .child(summary)
            .when(self.is_open, |this| {
                this.child(
                    div()
                        .mt_2()
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .bg(palette.surface_alt)
                        .rounded(px(RADIUS_SM))
                        .child(self.render_actions(busy, has_invalid, palette, cx))
                        .child(self.render_general(&general_issues, show_issues, palette))
                        .child(self.render_tuning(tunable_sensors, show_issues, palette)),
                )
            })
    }
}

/// Runs one queued operation to completion. `None` when the panel is gone.
async fn run_operation(
    this: &WeakEntity<SensorSettings>,
    operation: SettingsOperation,
    cx: &mut AsyncWindowContext,
) -> Option<()> {
    match operation {
        SettingsOperation::RestoreDefaults => this
            .update_in(cx, |this, window, cx| {
                this.form.restore_defaults(this.target.tunable_sensors());
                this.push_all(window, cx);
                cx.notify();
            })
            .ok(),
        SettingsOperation::Load | SettingsOperation::LoadSensors => {
            let sensors_only = matches!(operation, SettingsOperation::LoadSensors);
            let job = this
                .update_in(cx, |this, _, cx| {
                    let client = SensorSettings::hub_client(cx);
                    let target = this.target.clone();
                    let job = this.form.start_load(&target, client, sensors_only);
                    cx.notify();
                    job
                })
                .ok()?;
            let outcome = runtime::spawn(read_from_device(job)).await;
            this.update_in(cx, |this, window, cx| {
                match outcome {
                    Ok(outcome) => {
                        this.form.finish_load(outcome);
                        this.push_all(window, cx);
                    }
                    Err(error) => {
                        tracing::error!("reading settings task failed: {error}");
                        this.form
                            .finish_write(rpc_failure("read the settings", &error));
                    }
                }
                cx.notify();
            })
            .ok()
        }
        SettingsOperation::Write(persist) => {
            let job = this
                .update_in(cx, |this, _, cx| {
                    let client = SensorSettings::hub_client(cx);
                    let target = this.target.clone();
                    let job = this.form.start_write(&target, client, persist);
                    cx.notify();
                    job
                })
                .ok()?;
            let Some(job) = job else {
                return Some(());
            };
            let status = runtime::spawn(write_to_device(job)).await.unwrap_or_else(|error| {
                tracing::error!("writing settings task failed: {error}");
                rpc_failure("write the settings", &error)
            });
            this.update_in(cx, |this, _, cx| {
                this.form.finish_write(status);
                cx.notify();
            })
            .ok()
        }
    }
}
