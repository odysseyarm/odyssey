//! The pairing commands of a row and the Pair/Cancel and clear-bonds buttons
//! that run them.

use gpui::{
    Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Task, Window, div,
};
use odyssey_hub_client::client::Client;
use tokio::sync::broadcast::error::RecvError;

use super::pairing_state::{NOT_CONNECTED, PairTarget, PairingState};
use crate::components::ui::{ButtonKind, button};
use crate::hub::Hub;
use crate::runtime;
use crate::theme::palette;

#[derive(Clone, Copy)]
enum Op {
    Start,
    Cancel,
    Clear,
}

/// Runs one hub call on the tokio runtime.
async fn call(op: Op, target: PairTarget, client: Client) -> Result<(), String> {
    runtime::spawn(async move {
        let result = match op {
            Op::Start => target.start(&client).await,
            Op::Cancel => target.cancel(&client).await,
            Op::Clear => target.clear(&client).await,
        };
        result.map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(std::convert::identity)
}

pub(super) struct PairingControls {
    hub: Entity<Hub>,
    target: PairTarget,
    state: PairingState,
    // Clearing forgets every bond, and re-pairing each device takes a trip to
    // it, so the first click only asks.
    confirming_clear: bool,
    /// Pairing results arrive once, so they come over their own channel.
    _results: Task<()>,
}

impl PairingControls {
    pub(super) fn new(hub: Entity<Hub>, target: PairTarget, cx: &mut Context<'_, Self>) -> Self {
        let mut results = hub.read(cx).subscribe_pairing();
        let watched = target.clone();
        let task = cx.spawn(async move |this, cx| {
            loop {
                let event = match results.recv().await {
                    Ok(event) => event,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                };
                let Some(status) = watched.result_of(&event) else {
                    continue;
                };
                let applied = this.update(cx, |this, cx| {
                    this.state.finished(status);
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        });
        Self {
            hub,
            target,
            state: PairingState::default(),
            confirming_clear: false,
            _results: task,
        }
    }

    /// Points the commands at the latest view of the same target.
    pub(super) fn retarget(&mut self, target: PairTarget) {
        self.target = target;
    }

    /// The last thing that happened, empty before the first command.
    pub(super) fn status(&self) -> &str {
        &self.state.status
    }

    /// Reserve the row before spawning: a cancel or clear must not overtake
    /// an in-flight start, and repeated clicks must not send duplicate commands.
    fn reserve(&mut self, cx: &mut Context<'_, Self>) -> bool {
        if self.state.command_pending {
            return false;
        }
        self.state.command_pending = true;
        cx.notify();
        true
    }

    fn release(&mut self, cx: &mut Context<'_, Self>) {
        self.state.command_pending = false;
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<'_, Self>) {
        if !self.reserve(cx) {
            return;
        }
        self.state.started();
        let client = self.hub.read(cx).client();
        let target = self.target.clone();
        cx.spawn(async move |this, cx| {
            let noun = target.pairing_noun();
            let outcome = match client {
                Some(client) => call(Op::Start, target, client).await,
                None => Err(NOT_CONNECTED.to_string()),
            };
            if let Err(error) = &outcome {
                tracing::error!("Failed to start {noun}: {error}");
            }
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = outcome {
                    this.state.start_failed(error);
                }
                this.release(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<'_, Self>) {
        if !self.reserve(cx) {
            return;
        }
        let client = self.hub.read(cx).client();
        let target = self.target.clone();
        cx.spawn(async move |this, cx| {
            let noun = target.pairing_noun();
            let outcome = match client {
                Some(client) => call(Op::Cancel, target, client).await,
                None => Err(NOT_CONNECTED.to_string()),
            };
            if let Err(error) = &outcome {
                tracing::error!("Failed to cancel {noun}: {error}");
            }
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(()) => this.state.cancelled(),
                    Err(error) => this.state.error(error),
                }
                this.release(cx);
            });
        })
        .detach();
    }

    fn clear(&mut self, cx: &mut Context<'_, Self>) {
        if !self.reserve(cx) {
            return;
        }
        let client = self.hub.read(cx).client();
        let was_pairing = self.state.pairing;
        let target = self.target.clone();
        cx.spawn(async move |this, cx| {
            let noun = target.pairing_noun();
            let bonds = target.bonds_noun();
            let message = target.cleared_message();
            let Some(client) = client else {
                tracing::warn!("Failed to clear {bonds}: {NOT_CONNECTED}");
                let _ = this.update(cx, |this, cx| {
                    this.state.error(NOT_CONNECTED);
                    this.release(cx);
                });
                return;
            };
            if was_pairing {
                if let Err(error) = call(Op::Cancel, target.clone(), client.clone()).await {
                    tracing::error!("Failed to cancel {noun} before clearing {bonds}: {error}");
                    let _ = this.update(cx, |this, cx| {
                        this.state.error(error);
                        this.release(cx);
                    });
                    return;
                }
                let _ = this.update(cx, |this, cx| {
                    this.state.stopped();
                    cx.notify();
                });
            }
            let outcome = call(Op::Clear, target, client).await;
            if let Err(error) = &outcome {
                tracing::error!("Failed to clear {bonds}: {error}");
            }
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(()) => this.state.bonds_cleared(message),
                    Err(error) => this.state.error(error),
                }
                this.release(cx);
            });
        })
        .detach();
    }
}

impl Render for PairingControls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let p = palette(cx);
        let id = cx.entity_id();
        let pending = self.state.command_pending;
        let row = div().flex().flex_shrink_0().items_center().gap_1();
        let label = self.target.clear_label();

        if self.confirming_clear {
            return row
                .child(
                    div()
                        .text_xs()
                        .text_color(p.fg)
                        .child(self.target.clear_prompt()),
                )
                .child(
                    button(("confirm-clear", id), label, ButtonKind::Danger, p)
                        .small()
                        .disabled(pending)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirming_clear = false;
                            this.clear(cx);
                        })),
                )
                .child(
                    button(("keep-bond", id), "Keep", ButtonKind::Secondary, p)
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirming_clear = false;
                            cx.notify();
                        })),
                );
        }

        let pair = if self.state.pairing {
            button(("cancel-pairing", id), "Cancel", ButtonKind::Secondary, p)
                .small()
                .disabled(pending)
                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
        } else {
            button(("start-pairing", id), "Pair", ButtonKind::Secondary, p)
                .small()
                .disabled(pending)
                .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
        };
        row.child(pair).child(
            button(("clear-bonds", id), label, ButtonKind::Secondary, p)
                .small()
                .disabled(pending)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.confirming_clear = true;
                    cx.notify();
                })),
        )
    }
}
