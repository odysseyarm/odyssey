//! Changes to the hub's accessory list.
//!
//! Only one whole-map replacement may be in flight at a time, so rapid edits
//! cannot overwrite each other's snapshots. Changes are shown only once the
//! hub accepts them.

use std::num::NonZeroU64;

use gpui::Context;
use odyssey_hub_common::accessory::{
    AccessoryInfo, AccessoryInfoMap, AccessoryRadio, AccessoryType,
};

use super::{AccessoriesView, PairingWindow, pairing_expiry, pairing_window, unix_time_ms};
use crate::runtime;

impl AccessoriesView {
    pub(super) fn add(
        &mut self,
        id: [u8; 6],
        name: String,
        ty: AccessoryType,
        radio: AccessoryRadio,
        cx: &mut Context<'_, Self>,
    ) {
        let mut info = AccessoryInfo::with_defaults(name, ty);
        info.radio = radio;
        self.enroll(id, info, cx);
    }

    pub(super) fn enroll(&mut self, id: [u8; 6], info: AccessoryInfo, cx: &mut Context<'_, Self>) {
        if self.edit_pending {
            return;
        }
        if info.ty == AccessoryType::MarkerControlBoard
            && pairing_window(
                pairing_expiry(&self.hub.read(cx).accessory_radios, &info.radio, &id),
                unix_time_ms(),
            ) != PairingWindow::Open
        {
            self.edit_error = Some(
                "Marker board is not reporting an open pairing window. Hold its power button for 3 seconds, then wait for Pairing window open.".into(),
            );
            cx.notify();
            return;
        }
        let Some(client) = self.hub.read(cx).client() else {
            self.edit_error = Some("Cannot enroll accessory: hub is disconnected.".into());
            cx.notify();
            return;
        };
        self.edit_pending = true;
        self.enrolling = Some(id);
        self.edit_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let sent = info.clone();
            let outcome = runtime::spawn(async move {
                client.enroll_accessory(id, sent).await.map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(std::convert::identity);
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(()) => {
                        this.infos.insert(id, info);
                        this.radio_drafts.remove(&id);
                    }
                    Err(error) => {
                        let hint = if info.ty == AccessoryType::MarkerControlBoard {
                            " If the physical pairing window closed, hold the board’s power button for 3 seconds and wait for Pairing window open before retrying."
                        } else {
                            ""
                        };
                        this.edit_error = Some(format!(
                            "Enrollment failed: {error}. Previous radio assignment is unchanged.{hint}"
                        ));
                        this.radio_drafts.remove(&id);
                        this.forget_shown_assignments();
                    }
                }
                this.edit_pending = false;
                this.enrolling = None;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn reset_marker_mode(&mut self, id: [u8; 6], cx: &mut Context<'_, Self>) {
        if self.edit_pending {
            return;
        }
        let Some(client) = self.hub.read(cx).client() else {
            self.edit_error = Some("Cannot reset marker mode: hub is disconnected.".into());
            cx.notify();
            return;
        };
        self.edit_pending = true;
        self.resetting = Some(id);
        self.edit_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let outcome = runtime::spawn(async move {
                client
                    .reset_marker_mode(id)
                    .await
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(std::convert::identity);
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = outcome {
                    this.edit_error = Some(format!("Reset to Auto failed: {error}"));
                }
                this.edit_pending = false;
                this.resetting = None;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn remove(&mut self, id: [u8; 6], cx: &mut Context<'_, Self>) {
        self.commit("remove", cx, |infos| {
            infos.remove(&id);
        });
    }

    /// Hands the accessory to the device with this uuid, or to none.
    pub(super) fn assign(
        &mut self,
        id: [u8; 6],
        device_uuid: Option<NonZeroU64>,
        cx: &mut Context<'_, Self>,
    ) {
        self.commit("assign", cx, |infos| {
            if let Some(info) = infos.get_mut(&id) {
                info.assignment = device_uuid;
            }
        });
    }

    pub(super) fn assign_screen(
        &mut self,
        id: [u8; 6],
        screen_id: Option<u8>,
        cx: &mut Context<'_, Self>,
    ) {
        self.commit("assign screen", cx, |infos| {
            if let Some(info) = infos.get_mut(&id) {
                info.screen_id = screen_id;
            }
        });
    }

    fn commit(
        &mut self,
        action: &'static str,
        cx: &mut Context<'_, Self>,
        edit: impl FnOnce(&mut AccessoryInfoMap),
    ) {
        if self.edit_pending {
            return;
        }
        self.edit_pending = true;
        self.edit_error = None;
        let mut next = self.infos.clone();
        edit(&mut next);

        let Some(client) = self.hub.read(cx).client() else {
            tracing::warn!("Failed to {action} accessory: not connected to the hub");
            self.edit_pending = false;
            self.edit_error = Some(format!("Cannot {action} accessory: hub is disconnected."));
            self.forget_shown_assignments();
            cx.notify();
            return;
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let sent = next.clone();
            let outcome = runtime::spawn(async move {
                client
                    .update_accessory_info_map(sent)
                    .await
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(std::convert::identity);
            // The hub publishes the canonical map for an accepted write, so applying
            // `next` here would drop accessories it added while the call was in flight.
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = outcome {
                    tracing::error!("Failed to {action} accessory: {error}");
                    this.edit_error = Some(format!("Failed to {action} accessory: {error}"));
                    this.forget_shown_assignments();
                }
                this.edit_pending = false;
                cx.notify();
            });
        })
        .detach();
    }
}
