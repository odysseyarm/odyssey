use gpui::component::input::{InputEvent, InputState};
use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Window, div,
};
use odyssey_hub_client::client::Client;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::TrackingEvent;
use tokio_util::sync::CancellationToken;

use super::device_state::{DeviceState, Failure, Operation};
use crate::components::ui::{AlertKind, ButtonKind, alert, button};
use crate::hub::Hub;
use crate::runtime;
use crate::theme::palette;
use crate::workspace::{Workspace, WorkspaceHandle};

pub(super) struct DeviceControls {
    pub(super) device: Device,
    pub(super) hub: Entity<Hub>,
    workspace: Entity<Workspace>,
    pub(super) state: DeviceState,
    pub(super) ready: bool,
    pub(super) connected: bool,
    pub(super) shot_delay_input: Entity<InputState>,
    pub(super) shown_revision: u64,
    pub(super) pending: bool,
    pub(super) delay_loaded: bool,
    pub(super) delay_error: Option<String>,
    pub(super) delay_notice: Option<&'static str>,
    advanced: bool,
    confirmation: Option<Operation>,
    pub(super) disconnected: CancellationToken,
    input_events: Subscription,
}

impl DeviceControls {
    pub(super) fn new(
        device: Device,
        hub: Entity<Hub>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let workspace = cx.global::<WorkspaceHandle>().0.clone();
        let state = DeviceState::new(workspace.read(cx).alignment(device.uuid));
        let (shot_delay_input, input_events) = Self::bind_input(state.shot_delay_ms(), window, cx);
        let mut controls = Self {
            device,
            hub,
            workspace,
            shown_revision: state.shot_delay_revision(),
            state,
            ready: false,
            connected: true,
            shot_delay_input,
            pending: false,
            delay_loaded: false,
            delay_error: None,
            delay_notice: None,
            advanced: false,
            confirmation: None,
            disconnected: CancellationToken::new(),
            input_events,
        };
        controls.load_shot_delay(cx);
        controls
    }

    fn bind_input(
        delay_ms: u16,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Subscription) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(delay_ms.to_string())
                .validate(|typed, _| typed.chars().all(|c| c.is_ascii_digit()))
        });
        let events = cx.subscribe_in(&input, window, |controls, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                controls.shot_delay_typed(window, cx);
            }
        });
        (input, events)
    }

    pub(super) fn rebind_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (input, events) = Self::bind_input(self.state.shot_delay_ms(), window, cx);
        self.shot_delay_input = input;
        self.input_events = events;
        self.shown_revision = self.state.shot_delay_revision();
        self.ready = false;
        self.state.cancel("Waiting for shot cancelled.");
        self.confirmation = None;
        if self.connected {
            self.load_shot_delay(cx);
        }
    }

    pub(super) fn sync_connection(&mut self, device: Option<Device>, cx: &mut Context<Self>) {
        let connected = device.is_some();
        if let Some(device) = device {
            self.device = device;
        }
        if self.connected != connected {
            self.connected = connected;
            self.ready = false;
            self.state
                .cancel("Tracker disconnected. Waiting for shot cancelled.");
            if connected {
                self.disconnected = CancellationToken::new();
                self.load_shot_delay(cx);
            } else {
                self.disconnected.cancel();
                self.delay_loaded = false;
                self.delay_error =
                    Some("Tracker disconnected. Reload shot delay after reconnecting.".to_owned());
            }
            cx.notify();
        }
    }

    pub(super) fn client(&mut self, cx: &mut Context<Self>) -> Option<Client> {
        let client = self.hub.read(cx).client().filter(|_| self.connected);
        if client.is_none() {
            self.state.error = Some("Tracker or hub disconnected. Reconnect and retry.".to_owned());
            cx.notify();
        }
        client
    }

    pub(super) fn start_operation(
        &mut self,
        operation: Operation,
        sample: Option<TrackingEvent>,
        cx: &mut Context<Self>,
    ) {
        if self.pending || self.state.pending.is_some() || !self.connected {
            return;
        }
        if operation == Operation::Apply && !self.ready {
            return;
        }
        let Some(client) = self.client(cx) else {
            return;
        };
        if !self.state.begin(operation) {
            return;
        }
        self.confirmation = None;
        cx.notify();
        let reply = runtime::spawn(super::zeroing::run_operation(
            client,
            self.device.clone(),
            operation,
            sample,
            self.disconnected.clone(),
        ));
        // Keep acknowledgement handling alive even if the user explicitly leaves mid-operation.
        let controls = cx.entity();
        cx.spawn(async move |_, cx| {
            let result = reply.await.unwrap_or_else(|error| {
                Err(Failure {
                    message: format!("Operation stopped: {error}. Result unknown; retry."),
                    unknown: true,
                })
            });
            controls.update(cx, |controls, cx| {
                let update_alignment =
                    result.is_ok() || result.as_ref().is_err_and(|failure| failure.unknown);
                controls.state.finish(operation, result);
                if update_alignment {
                    controls.workspace.update(cx, |workspace, cx| {
                        workspace.set_alignment(controls.device.uuid, controls.state.alignment, cx);
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        match self.state.retry {
            Some(Operation::Apply) => {
                self.state.error = None;
                self.state.retry = None;
                self.state.notice = Some("Aim at the center, then press Z to capture again.");
            }
            Some(operation @ (Operation::Reset | Operation::Clear)) => {
                self.confirmation = Some(operation);
            }
            Some(Operation::Save) => self.start_operation(Operation::Save, None, cx),
            None => {}
        }
        cx.notify();
    }
}

impl Render for DeviceControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &palette(cx);
        self.show_shot_delay(window, cx);
        let busy = self.pending || self.state.pending.is_some();
        let mut body = div().flex().flex_col().gap_3();
        if let Some(operation) = self.state.pending {
            body = body.child(alert(AlertKind::Info, p, div().child(operation.label())));
        }
        if self.state.unsaved {
            body = body.child(alert(AlertKind::Warning, p, div().child(if self.state.unknown {
                "Alignment result unknown. Do not trust the previous saved state. Capture again before saving."
            } else {
                "Active alignment changed, not saved. Leaving will not persist this change."
            })));
            body = body.child(
                button("zero-save", "Save zero", ButtonKind::Primary, p)
                    .disabled(busy || !self.connected || self.state.unknown || self.state.armed)
                    .on_click(cx.listener(|controls, _, _, cx| {
                        controls.start_operation(Operation::Save, None, cx);
                    })),
            );
        }
        if let Some(notice) = self.state.notice {
            body = body.child(div().text_sm().text_color(p.fg_muted).child(notice));
        }
        if self.state.armed {
            body = body.child(alert(AlertKind::Info, p, div().child("Waiting for this tracker's next shot. Z captures now; Escape cancels waiting.")))
                .child(button("zero-cancel-shot", "Cancel waiting", ButtonKind::Secondary, p)
                    .on_click(cx.listener(|controls, _, _, cx| {
                        controls.state.cancel("Waiting for shot cancelled. No zero captured.");
                        cx.notify();
                    })));
        }
        if let Some(error) = &self.state.error {
            let mut content = div().flex().flex_col().gap_2().child(error.clone());
            if self.state.retry.is_some() {
                content = content.child(
                    button("zero-retry", "Retry", ButtonKind::Secondary, p)
                        .small()
                        .disabled(busy || !self.connected)
                        .on_click(cx.listener(|controls, _, _, cx| controls.retry(cx))),
                );
            }
            body = body.child(alert(AlertKind::Danger, p, content));
        }
        if let Some(error) = &self.delay_error {
            body = body.child(alert(
                AlertKind::Danger,
                p,
                div().flex().flex_col().gap_2().child(error.clone()).child(
                    button(
                        "zero-reload-delay",
                        "Reload shot delay",
                        ButtonKind::Secondary,
                        p,
                    )
                    .small()
                    .disabled(busy || !self.connected)
                    .on_click(cx.listener(|controls, _, _, cx| controls.load_shot_delay(cx))),
                ),
            ));
        }
        body = body.child(
            button(
                "zero-advanced",
                if self.advanced {
                    "Hide Advanced"
                } else {
                    "Advanced"
                },
                ButtonKind::Ghost,
                p,
            )
            .small()
            .on_click(cx.listener(|controls, _, _, cx| {
                controls.advanced = !controls.advanced;
                controls.confirmation = None;
                cx.notify();
            })),
        );
        if self.advanced {
            body = body.child(div().text_sm().text_color(p.fg_muted).child("Optional shot capture: arm only this tracker, then aim and fire. Never captures automatically from a steady pose."))
                .child(button("zero-arm-shot", "Zero on next shot", ButtonKind::Secondary, p)
                    .disabled(busy || !self.ready || self.state.armed)
                    .on_click(cx.listener(|controls, _, _, cx| {
                        controls.state.arm(controls.ready);
                        cx.notify();
                    })))
                .child(div().text_sm().text_color(p.fg_muted).child("Shot delay shifts capture backward from the shot timestamp (or newest sample for Z). Set changes active delay; Save sets then persists it; Reset restores the service default without saving."))
                .child(self.shot_delay_row(p, cx))
                .child(div().text_sm().text_color(p.fg_muted).child("Reset zero and Clear zero both remove this tracker's active offset in Odyssey. Neither persists removal until Save zero. Stored tracker firmware settings are unchanged."))
                .child(div().flex().flex_wrap().gap_2()
                    .child(button("zero-reset", "Reset zero…", ButtonKind::Secondary, p).small()
                        .disabled(busy || !self.connected || self.state.armed)
                        .on_click(cx.listener(|controls, _, _, cx| { controls.confirmation = Some(Operation::Reset); cx.notify(); })))
                    .child(button("zero-clear", "Clear zero…", ButtonKind::Secondary, p).small()
                        .disabled(busy || !self.connected || self.state.armed)
                        .on_click(cx.listener(|controls, _, _, cx| { controls.confirmation = Some(Operation::Clear); cx.notify(); }))));
            if let Some(notice) = self.delay_notice {
                body = body.child(div().text_sm().text_color(p.fg_muted).child(notice));
            }
            if let Some(operation) = self.confirmation {
                body = body.child(alert(AlertKind::Warning, p, div().flex().flex_col().gap_2()
                    .child("Remove active zero for this tracker? Check alignment again after removal. Existing saved zero remains until you save removal.")
                    .child(button("zero-confirm-remove", "Remove active zero", ButtonKind::Danger, p).small()
                        .disabled(busy || !self.connected)
                        .on_click(cx.listener(move |controls, _, _, cx| controls.start_operation(operation, None, cx))))
                    .child(button("zero-cancel-remove", "Keep zero", ButtonKind::Secondary, p).small()
                        .on_click(cx.listener(|controls, _, _, cx| { controls.confirmation = None; cx.notify(); })))));
            }
        }
        body
    }
}
