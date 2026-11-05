//! Changes to the hub's accessory list.
//!
//! Only one whole-map replacement may be in flight at a time, so rapid edits
//! cannot overwrite each other's snapshots. Changes are shown only once the
//! hub accepts them.

use std::num::NonZeroU64;

use gpui::Context;
use odyssey_hub_common::accessory::{AccessoryInfo, AccessoryInfoMap, AccessoryType};

use super::AccessoriesView;
use crate::runtime;

impl AccessoriesView {
    pub(super) fn add(
        &mut self,
        id: [u8; 6],
        name: String,
        ty: AccessoryType,
        cx: &mut Context<'_, Self>,
    ) {
        self.commit("add", cx, |infos| {
            infos.insert(id, AccessoryInfo::with_defaults(name, ty));
        });
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
        let mut next = self.infos.clone();
        edit(&mut next);

        let Some(client) = self.hub.read(cx).client() else {
            tracing::warn!("Failed to {action} accessory: not connected to the hub");
            self.edit_pending = false;
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
            // The hub replaces the whole map, so showing the edit before it
            // is accepted leaves the UI claiming a change the hub rejected.
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(()) => this.infos = next,
                    Err(error) => {
                        tracing::error!("Failed to {action} accessory: {error}");
                        this.forget_shown_assignments();
                    }
                }
                this.edit_pending = false;
                cx.notify();
            });
        })
        .detach();
    }
}
