//! Following the hub's accessory list.

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt as _;
use gpui::{Context, Task};
use odyssey_hub_client::client::{Client, ClientError, RpcError};
use odyssey_hub_common::accessory::{AccessoryInfo, AccessoryMap};
use tokio::sync::mpsc;

use super::AccessoriesView;
use crate::runtime;

/// How long to wait before subscribing again after the stream ended.
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(2);

/// The accessories the hub knows, and whether each is connected.
pub(super) type Known = HashMap<[u8; 6], (AccessoryInfo, bool)>;

/// Why the accessory map stopped updating.
enum StreamDown {
    NoHub,
    Subscribe(ClientError),
    Failed(RpcError),
    Ended,
}

/// Seeds the view's `known` and `infos` from the hub and keeps them current.
/// Following stops when the returned task is dropped.
pub(super) fn follow(cx: &mut Context<'_, AccessoriesView>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        let Ok(client) = this.update(cx, |this, cx| this.hub.read(cx).client()) else {
            return;
        };
        if let Some(client) = client
            && let Ok(Ok(map)) =
                runtime::spawn(async move { client.get_accessory_map().await }).await
            && this.update(cx, |this, cx| this.publish(map, cx)).is_err()
        {
            return;
        }

        // Resubscribe after a stream error: ending the loop would freeze the
        // list for the rest of the session.
        loop {
            // The client is taken afresh each attempt, so a retry picks up a
            // later connection.
            let Ok(client) = this.update(cx, |this, cx| this.hub.read(cx).client()) else {
                return;
            };
            let down = match client {
                None => StreamDown::NoHub,
                Some(client) => {
                    let (maps, mut received) = mpsc::unbounded_channel();
                    let stream = runtime::spawn(follow_accessory_map(client, maps));
                    while let Some(map) = received.recv().await {
                        if this.update(cx, |this, cx| this.publish(map, cx)).is_err() {
                            stream.abort();
                            return;
                        }
                    }
                    stream.await.unwrap_or(StreamDown::Ended)
                }
            };
            match down {
                StreamDown::NoHub => {
                    tracing::warn!("accessory map subscribe failed: not connected to the hub");
                }
                StreamDown::Subscribe(e) => tracing::warn!("accessory map subscribe failed: {e}"),
                StreamDown::Failed(e) => tracing::warn!("accessory map stream error: {e}"),
                StreamDown::Ended => {}
            }
            cx.background_executor().timer(RESUBSCRIBE_DELAY).await;
        }
    })
}

/// Forwards accessory maps until the subscription fails or ends; dropping the
/// sender then tells the receiver.
async fn follow_accessory_map(
    client: Client,
    maps: mpsc::UnboundedSender<AccessoryMap>,
) -> StreamDown {
    let mut stream = match client.subscribe_accessory_map().await {
        Ok(stream) => stream,
        Err(e) => return StreamDown::Subscribe(e),
    };
    while let Some(next) = stream.next().await {
        match next {
            Ok(map) => {
                if maps.send(map).is_err() {
                    break;
                }
            }
            Err(e) => return StreamDown::Failed(e),
        }
    }
    StreamDown::Ended
}
