//! Following the hub's accessory list.

use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;

use backon::{ConstantBuilder, Retryable as _};
use dioxus::logger::tracing;
use dioxus::prelude::*;
use futures::StreamExt as _;
use odyssey_hub_client::client::{ClientError, RpcError};
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
        let client = hub.read().client.read().clone();
        if let Some(client) = client
            && let Ok(map) = client.get_accessory_map().await
        {
            publish(map, &mut known, &mut infos);
        }

        // Resubscribe after a stream error: ending the loop would freeze the
        // list for the rest of the session.
        let backoff = ConstantBuilder::new()
            .with_delay(RESUBSCRIBE_DELAY)
            .without_max_times();
        // A stream never ends well, so with no retry limit this only
        // completes by being dropped.
        let _ = (|| follow_accessory_map(hub, known, infos))
            .retry(backoff)
            .notify(|down, _| match down {
                StreamDown::NoHub => {
                    tracing::warn!("accessory map subscribe failed: not connected to the hub");
                }
                StreamDown::Subscribe(e) => tracing::warn!("accessory map subscribe failed: {e}"),
                StreamDown::Failed(e) => tracing::warn!("accessory map stream error: {e}"),
                StreamDown::Ended => {}
            })
            .await;
    });
}

/// Why the accessory map stopped updating.
enum StreamDown {
    NoHub,
    Subscribe(ClientError),
    Failed(RpcError),
    Ended,
}

/// Applies accessory maps until the subscription fails or ends. Takes the
/// hub's client afresh each attempt, so a retry picks up a later connection.
async fn follow_accessory_map(
    hub: Signal<HubContext>,
    mut known: Signal<Known>,
    mut infos: Signal<AccessoryInfoMap>,
) -> Result<Infallible, StreamDown> {
    let Some(client) = hub.read().client.read().clone() else {
        return Err(StreamDown::NoHub);
    };
    let mut stream = client
        .subscribe_accessory_map()
        .await
        .map_err(StreamDown::Subscribe)?;
    while let Some(next) = stream.next().await {
        let map = next.map_err(StreamDown::Failed)?;
        publish(map, &mut known, &mut infos);
    }
    Err(StreamDown::Ended)
}

fn publish(map: AccessoryMap, known: &mut Signal<Known>, infos: &mut Signal<AccessoryInfoMap>) {
    let info_only = map
        .iter()
        .map(|(id, (info, _))| (*id, info.clone()))
        .collect();
    known.set(map);
    infos.set(info_only);
}
