//! The button that stores the current zero on the device.

use std::time::Duration;

use gpui::{Context, IntoElement, StatefulInteractiveElement as _};

use super::device_controls::{DeviceControls, Tone, pill};
use super::device_state::SaveStatus;
use crate::runtime;
use crate::theme::Palette;

/// How long "Saved" stays on the button before it goes back to idle.
const SAVED_SHOWN_FOR: Duration = Duration::from_secs(2);
/// How long an error stays on the button, long enough to read.
const ERROR_SHOWN_FOR: Duration = Duration::from_secs(4);

impl DeviceControls {
    pub(super) fn save_zero_button(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let (tone, label) = match &self.state.save_zero_status {
            None => (Tone::Plain, "Save Zero".to_owned()),
            Some(Ok(())) => (Tone::Success, "Saved".to_owned()),
            Some(Err(e)) => (Tone::Danger, format!("Error: {e}")),
        };
        pill(("zero-save", cx.entity_id()), label, tone, self.saving, p)
            .on_click(cx.listener(|controls, _, _, cx| controls.save_zero(cx)))
    }

    /// Shows `status` and restarts the timer that clears it. Replacing the
    /// task cancels the previous timer, so it cannot clear a newer result too
    /// early.
    fn set_save_status(&mut self, status: SaveStatus, cx: &mut Context<Self>) {
        let shown_for = match &status {
            None => None,
            Some(Ok(())) => Some(SAVED_SHOWN_FOR),
            Some(Err(_)) => Some(ERROR_SHOWN_FOR),
        };
        self.state.save_zero_status = status;
        self.save_status_timer = shown_for.map(|shown_for| {
            let timer = cx.background_executor().timer(shown_for);
            cx.spawn(async move |this, cx| {
                timer.await;
                this.update(cx, |controls, cx| controls.set_save_status(None, cx))
                    .ok();
            })
        });
        cx.notify();
    }

    fn save_zero(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(client) = self.hub.read(cx).client() else {
            let message = "not connected to the hub".to_owned();
            tracing::warn!("Failed to save zero {:x?}: {}", self.device.uuid, message);
            self.set_save_status(Some(Err(message)), cx);
            return;
        };
        self.saving = true;
        cx.notify();
        let device = self.device.clone();
        let saved = runtime::spawn(async move {
            match client.device(device.clone()).save_zero().await {
                Ok(()) => {
                    tracing::info!("Saved zero for {:x?}", device.uuid);
                    Ok(())
                }
                Err(e) => {
                    let message = e.to_string();
                    tracing::error!("Failed to save zero {:x?}: {}", device.uuid, message);
                    Err(message)
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let status = saved.await.unwrap_or_else(|e| Err(e.to_string()));
            this.update(cx, |controls, cx| {
                controls.set_save_status(Some(status), cx);
                controls.saving = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
