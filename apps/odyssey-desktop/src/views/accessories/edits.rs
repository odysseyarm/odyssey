//! Changes to the hub's accessory list.

use std::num::NonZeroU64;

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::accessory::{AccessoryInfo, AccessoryInfoMap, AccessoryType};

use crate::hub::HubContext;

/// The ways the list can be edited. Each edit sends the whole changed list to
/// the hub and shows it only once the hub accepts it.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct AccessoryEdits {
    hub: Signal<HubContext>,
    infos: Signal<AccessoryInfoMap>,
}

impl AccessoryEdits {
    pub(super) fn new(hub: Signal<HubContext>, infos: Signal<AccessoryInfoMap>) -> Self {
        Self { hub, infos }
    }

    pub(super) fn add(self, id: [u8; 6], name: String, ty: AccessoryType) {
        self.commit("add", |infos| {
            infos.insert(id, AccessoryInfo::with_defaults(name, ty));
        });
    }

    pub(super) fn remove(self, id: [u8; 6]) {
        self.commit("remove", |infos| {
            infos.remove(&id);
        });
    }

    /// Hands the accessory to the device with this uuid, or to none.
    pub(super) fn assign(self, id: [u8; 6], device_uuid: Option<NonZeroU64>) {
        self.commit("assign", |infos| {
            if let Some(info) = infos.get_mut(&id) {
                info.assignment = device_uuid;
            }
        });
    }

    fn commit(mut self, action: &'static str, edit: impl FnOnce(&mut AccessoryInfoMap)) {
        let mut next = self.infos.read().clone();
        edit(&mut next);

        let mut client = self.hub.read().client.read().clone();
        spawn(async move {
            // The hub replaces the whole map, so showing the edit before it
            // is accepted leaves the UI claiming a change the hub rejected.
            match client.update_accessory_info_map(next.clone()).await {
                Ok(()) => self.infos.set(next),
                Err(e) => tracing::error!("Failed to {action} accessory: {e}"),
            }
        });
    }
}
