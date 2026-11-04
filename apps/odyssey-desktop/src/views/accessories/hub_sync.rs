//! Following the hub's accessory list.

use std::collections::HashMap;
use std::time::Duration;

use dioxus::logger::tracing;
use dioxus::prelude::*;
use futures::StreamExt as _;
use odyssey_hub_common::accessory::{AccessoryInfo, AccessoryInfoMap, AccessoryMap};

use crate::hub::HubContext;

/// How long to wait before subscribing again after the stream ended.
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(2);

/// The accessories the hub knows, and whether each is connected.
pub(super) type Known = HashMap<[u8; 6], (AccessoryInfo, bool)>;

/// Seeds `known` and `infos` from the hub and keeps them current.
pub(super) fn use_accessory_map(
    hub: Signal<HubContext>,
    mut known: Signal<Known>,
    mut infos: Signal<AccessoryInfoMap>,
) {
    use_future(move || async move {
        let mut client = hub.read().client.read().clone();

        if let Ok(map) = client.get_accessory_map().await {
            publish(map, &mut known, &mut infos);
        }

        // Resubscribe after a stream error: ending the loop would freeze the
        // list for the rest of the session.
        loop {
            match client.subscribe_accessory_map().await {
                Ok(mut stream) => {
                    while let Some(next) = stream.next().await {
                        match next {
                            Ok(map) => publish(map, &mut known, &mut infos),
                            Err(e) => {
                                tracing::warn!("accessory map stream error: {e}");
                                break;
                            }
                        }
                    }
                }
                Err(e) => tracing::warn!("accessory map subscribe failed: {e}"),
            }
            tokio::time::sleep(RESUBSCRIBE_DELAY).await;
        }
    });
}

fn publish(map: AccessoryMap, known: &mut Signal<Known>, infos: &mut Signal<AccessoryInfoMap>) {
    let info_only = map
        .iter()
        .map(|(id, (info, _))| (*id, info.clone()))
        .collect();
    known.set(map);
    infos.set(info_only);
}
