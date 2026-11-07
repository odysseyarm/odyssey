use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt;
use std::pin::pin;
use std::time::Duration;

use backon::{ConstantBuilder, Retryable as _};
use futures::{Stream, StreamExt as _, stream};
use futures_concurrency::stream::Merge as _;
use gpui::{Context, Entity, Global};
use odyssey_hub_client::client::{Client, DongleInfo, RpcError};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events as oe;
use tokio::sync::{broadcast, mpsc, watch};

use crate::runtime;

/// How long to wait before dialling the hub server again after a session ends
/// or a connection attempt fails.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

const TRACKING_CHANNEL_CAPACITY: usize = 128;
const IMPACT_CHANNEL_CAPACITY: usize = 64;
const PAIRING_CHANNEL_CAPACITY: usize = 16;

/// The hub connection as the UI sees it. One entity per app, reachable from any
/// view through [`HubHandle`].
pub(crate) struct Hub {
    /// `None` until the first connection to the hub server succeeds.
    pub client: Option<Client>,
    pub devices: Vec<Device>,
    pub dongles: Vec<DongleInfo>,
    /// Most recent battery state (percent, charging) per device UUID.
    pub battery_states: HashMap<[u8; 6], (u8, bool)>,
    tracking_events: broadcast::Sender<(Device, oe::TrackingEvent)>,
    impact_events: broadcast::Sender<(Device, oe::ImpactEvent)>,
    /// One-shot results a row has to observe exactly once.
    pairing_events: broadcast::Sender<oe::Event>,
}

/// The app-wide hub entity.
#[derive(Clone)]
pub(crate) struct HubHandle(pub Entity<Hub>);

impl Global for HubHandle {}

/// One of the three server-side streams the hub follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamKind {
    DeviceList,
    DongleList,
    Events,
}

impl fmt::Display for StreamKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DeviceList => "device list",
            Self::DongleList => "dongle list",
            Self::Events => "event",
        })
    }
}

/// One message from the merged subscription stream, tagged with its origin.
// `Event` carries a whole device event by value: it is moved once from the stream
// into the loop, and boxing would add an allocation to every tracking event.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "boxing would allocate once per tracking event, the hot path"
)]
enum HubUpdate {
    Devices(Result<Vec<Device>, RpcError>),
    Dongles(Result<Vec<DongleInfo>, RpcError>),
    Event(Result<oe::Event, RpcError>),
    /// A stream ran dry. `Merge` keeps polling the others after one ends, so
    /// the end has to be an item of its own for the session to notice it.
    Closed(StreamKind),
}

/// Why a session stopped consuming updates.
enum SessionEnd {
    StreamFailed(StreamKind, RpcError),
    StreamClosed(StreamKind),
}

/// Why the hub stopped following the server.
enum LinkDown {
    Connect(eros::ErrorUnion),
    Session(SessionEnd),
}

/// A connected hub: the device list as of connecting, plus every later update.
struct Subscriptions<Updates> {
    initial_devices: Vec<Device>,
    updates: Updates,
}

/// What the tokio side tells the model.
enum Change {
    Connected(Client),
    Devices(Vec<Device>),
    Dongles(Vec<DongleInfo>),
    /// The only device event that changes model state; the rest go over the
    /// broadcast channels so the UI is not woken at IMU rate.
    Battery {
        uuid: [u8; 6],
        percent: u8,
        charging: bool,
    },
}

/// Tags a stream's items with `wrap` and appends a [`HubUpdate::Closed`] marker.
fn tagged<Item>(
    source: impl Stream<Item = Result<Item, RpcError>>,
    kind: StreamKind,
    wrap: fn(Result<Item, RpcError>) -> HubUpdate,
) -> impl Stream<Item = HubUpdate> {
    source
        .map(wrap)
        .chain(stream::once(std::future::ready(HubUpdate::Closed(kind))))
}

/// Interleaves the three server streams into one. All items already share the
/// [`HubUpdate`] type, so the differently-typed streams need no boxing.
fn merged_updates(
    devices: impl Stream<Item = Result<Vec<Device>, RpcError>>,
    dongles: impl Stream<Item = Result<Vec<DongleInfo>, RpcError>>,
    events: impl Stream<Item = Result<oe::Event, RpcError>>,
) -> impl Stream<Item = HubUpdate> {
    (
        tagged(devices, StreamKind::DeviceList, HubUpdate::Devices),
        tagged(dongles, StreamKind::DongleList, HubUpdate::Dongles),
        tagged(events, StreamKind::Events, HubUpdate::Event),
    )
        .merge()
}

/// Connects to the hub and subscribes to every stream the hub needs. The client
/// is a plain value, so nothing is locked across the awaits.
async fn connect() -> eros::Result<(Client, Subscriptions<impl Stream<Item = HubUpdate> + use<>>)> {
    let client =
        eros::Context::context(Client::connect().await, "Failed to connect to hub server")?;
    tracing::info!("Hub connected successfully");

    let initial_devices =
        eros::Context::context(client.get_device_list().await, "Failed to get device list")?;
    let devices = eros::Context::context(
        client.subscribe_device_list().await,
        "Failed to subscribe to device list",
    )?;
    let dongles = eros::Context::context(
        client.subscribe_dongle_list().await,
        "Failed to subscribe to dongle list",
    )?;
    let events = eros::Context::context(
        client.subscribe_events().await,
        "Failed to subscribe to events",
    )?;

    Ok((
        client,
        Subscriptions {
            initial_devices,
            updates: merged_updates(devices, dongles, events),
        },
    ))
}

/// Whether `event` is a one-shot pairing outcome that row components wait for.
fn is_pairing_result(event: &oe::Event) -> bool {
    matches!(
        event,
        oe::Event::DonglePairingResult { .. }
            | oe::Event::DeviceEvent(oe::DeviceEvent(
                _,
                oe::DeviceEventKind::PairingResult { .. }
            ))
    )
}

/// The tokio half: follows the server and forwards changes to the model.
#[derive(Clone)]
struct Follower {
    changes: mpsc::UnboundedSender<Change>,
    tracking_events: broadcast::Sender<(Device, oe::TrackingEvent)>,
    impact_events: broadcast::Sender<(Device, oe::ImpactEvent)>,
    pairing_events: broadcast::Sender<oe::Event>,
}

impl Follower {
    /// Follows the hub server for as long as the future lives, reconnecting
    /// whenever a session ends.
    async fn run(self, mut server_ready: watch::Receiver<bool>) {
        // The server has to be listening before the first dial.
        if server_ready.wait_for(|ready| *ready).await.is_err() {
            return;
        }
        let backoff = ConstantBuilder::new()
            .with_delay(RECONNECT_DELAY)
            .without_max_times();
        // A session never ends well, so with no retry limit this only
        // completes by being dropped.
        let _ = (|| {
            let follower = self.clone();
            async move { follower.follow_server().await }
        })
        .retry(backoff)
        .notify(|down, delay| match down {
            LinkDown::Connect(error) => tracing::error!("{error:?}"),
            LinkDown::Session(SessionEnd::StreamFailed(stream, error)) => {
                tracing::error!("{stream} stream error: {error}");
                tracing::warn!("Reconnecting in {delay:?}...");
            }
            LinkDown::Session(SessionEnd::StreamClosed(stream)) => {
                tracing::warn!("{stream} stream ended");
                tracing::warn!("Reconnecting in {delay:?}...");
            }
        })
        .await;
    }

    /// Dials the server and applies its updates until the session ends.
    async fn follow_server(&self) -> Result<Infallible, LinkDown> {
        tracing::info!("Hub connecting to server...");
        let (client, subscriptions) = connect().await.map_err(LinkDown::Connect)?;
        // Publish only once connected, so readers never see a half-dialled
        // client.
        let _ = self.changes.send(Change::Connected(client));
        Err(LinkDown::Session(self.run_session(subscriptions).await))
    }

    /// Applies updates until any stream fails or ends; the stale half of a
    /// broken subscription must not keep serving the UI.
    async fn run_session(
        &self,
        subscriptions: Subscriptions<impl Stream<Item = HubUpdate>>,
    ) -> SessionEnd {
        let _ = self
            .changes
            .send(Change::Devices(subscriptions.initial_devices));

        let mut updates = pin!(subscriptions.updates);
        while let Some(update) = updates.next().await {
            match update {
                HubUpdate::Devices(Ok(list)) => {
                    tracing::debug!("Device list update: {} devices", list.len());
                    let _ = self.changes.send(Change::Devices(list));
                }
                HubUpdate::Dongles(Ok(list)) => {
                    tracing::debug!("Dongle list update: {} dongles", list.len());
                    let _ = self.changes.send(Change::Dongles(list));
                }
                HubUpdate::Event(Ok(event)) => self.handle_event(event),
                HubUpdate::Devices(Err(error)) => {
                    return SessionEnd::StreamFailed(StreamKind::DeviceList, error);
                }
                HubUpdate::Dongles(Err(error)) => {
                    return SessionEnd::StreamFailed(StreamKind::DongleList, error);
                }
                HubUpdate::Event(Err(error)) => {
                    return SessionEnd::StreamFailed(StreamKind::Events, error);
                }
                HubUpdate::Closed(stream) => return SessionEnd::StreamClosed(stream),
            }
        }
        // Every stream ends with a `Closed` marker, which returns above.
        unreachable!("merged stream ended without a Closed marker")
    }

    fn handle_event(&self, event: oe::Event) {
        tracing::trace!("Hub event: {:?}", event);
        if let oe::Event::DeviceEvent(oe::DeviceEvent(device, kind)) = &event {
            match kind {
                oe::DeviceEventKind::TrackingEvent(tracking) => {
                    // No subscribers is the normal case outside the zero view.
                    // Tracking never reaches the model: it would wake the UI at
                    // the camera frame rate.
                    let _ = self.tracking_events.send((device.clone(), *tracking));
                    return;
                }
                oe::DeviceEventKind::ImpactEvent(impact) => {
                    let _ = self.impact_events.send((device.clone(), *impact));
                }
                _ => {}
            }
        }
        if is_pairing_result(&event) {
            let _ = self.pairing_events.send(event.clone());
        }
        if let oe::Event::DeviceEvent(oe::DeviceEvent(
            device,
            oe::DeviceEventKind::BatteryEvent(battery),
        )) = &event
        {
            let _ = self.changes.send(Change::Battery {
                uuid: device.uuid,
                percent: battery.percent,
                charging: battery.charging,
            });
        }
    }
}

impl Hub {
    /// Creates the model and starts following the server once `server_ready`
    /// turns true. The follower task ends when the model is dropped.
    pub(crate) fn new(server_ready: watch::Receiver<bool>, cx: &mut Context<Self>) -> Self {
        let (tracking_events, _) = broadcast::channel(TRACKING_CHANNEL_CAPACITY);
        let (impact_events, _) = broadcast::channel(IMPACT_CHANNEL_CAPACITY);
        let (pairing_events, _) = broadcast::channel(PAIRING_CHANNEL_CAPACITY);
        let (changes, mut receiver) = mpsc::unbounded_channel();

        let follower = Follower {
            changes,
            tracking_events: tracking_events.clone(),
            impact_events: impact_events.clone(),
            pairing_events: pairing_events.clone(),
        };
        let task = runtime::spawn(follower.run(server_ready));

        cx.spawn(async move |this, cx| {
            // Aborts the follower when the model drops this task.
            let _stop = AbortOnDrop(task);
            while let Some(change) = receiver.recv().await {
                let applied = this.update(cx, |hub, cx| hub.apply(change, cx));
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();

        Self {
            client: None,
            devices: Vec::new(),
            dongles: Vec::new(),
            battery_states: HashMap::new(),
            tracking_events,
            impact_events,
            pairing_events,
        }
    }

    fn apply(&mut self, change: Change, cx: &mut Context<Self>) {
        match change {
            Change::Connected(client) => self.client = Some(client),
            Change::Devices(list) => self.devices = list,
            Change::Dongles(list) => self.dongles = list,
            Change::Battery {
                uuid,
                percent,
                charging,
            } => {
                tracing::info!(
                    "BatteryEvent received for {uuid:02x?}: {percent}% charging={charging}"
                );
                self.battery_states.insert(uuid, (percent, charging));
            }
        }
        cx.notify();
    }

    /// The connected client, or `None` before the hub server first answers.
    pub(crate) fn client(&self) -> Option<Client> {
        self.client.clone()
    }

    pub(crate) fn subscribe_tracking(&self) -> broadcast::Receiver<(Device, oe::TrackingEvent)> {
        self.tracking_events.subscribe()
    }

    pub(crate) fn subscribe_impact(&self) -> broadcast::Receiver<(Device, oe::ImpactEvent)> {
        self.impact_events.subscribe()
    }

    pub(crate) fn subscribe_pairing(&self) -> broadcast::Receiver<oe::Event> {
        self.pairing_events.subscribe()
    }
}

/// Aborts a tokio task when dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list stream that ends must surface as a `Closed` item even while the
    /// other streams stay open; this is what turns a silent stale list into a
    /// reconnect.
    #[tokio::test]
    async fn ended_stream_is_reported_while_others_stay_open() {
        let devices = stream::iter([Ok::<Vec<Device>, RpcError>(Vec::new())]);
        let dongles = stream::pending::<Result<Vec<DongleInfo>, RpcError>>();
        let events = stream::pending::<Result<oe::Event, RpcError>>();

        let updates: Vec<HubUpdate> = merged_updates(devices, dongles, events)
            .take(2)
            .collect()
            .await;

        assert!(matches!(
            updates.as_slice(),
            [
                HubUpdate::Devices(Ok(list)),
                HubUpdate::Closed(StreamKind::DeviceList)
            ] if list.is_empty()
        ));
    }
}
