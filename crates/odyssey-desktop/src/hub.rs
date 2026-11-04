use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt;
use std::pin::pin;
use std::time::Duration;

use backon::{ConstantBuilder, Retryable as _};
use dioxus::logger::tracing;
use dioxus::prelude::*;
use futures::{Stream, StreamExt as _, stream};
use futures_concurrency::stream::Merge as _;
use odyssey_hub_client::client::{Client, DongleInfo, RpcError};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events as oe;
use slab::Slab;
use tokio::sync::broadcast;

/// How long to wait before dialling the hub server again after a session ends
/// or a connection attempt fails.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

const TRACKING_CHANNEL_CAPACITY: usize = 128;
const IMPACT_CHANNEL_CAPACITY: usize = 64;
const PAIRING_CHANNEL_CAPACITY: usize = 16;

#[derive(Clone)]
pub(crate) struct HubContext {
    /// `None` until the first connection to the hub server succeeds.
    pub client: SyncSignal<Option<Client>>,
    pub devices: SyncSignal<Slab<Device>>,
    pub dongles: SyncSignal<Vec<DongleInfo>>,
    /// Not `Sync`, so it is reactive only inside the main window's VirtualDom. A
    /// separate window such as Zero must use a broadcast channel instead.
    pub latest_event: Signal<Option<oe::Event>>,
    pub tracking_events: broadcast::Sender<(Device, oe::TrackingEvent)>,
    pub impact_events: broadcast::Sender<(Device, oe::ImpactEvent)>,
    /// One-shot results a row has to observe exactly once. They cannot ride
    /// `latest_event`: it holds only the newest event, and tracking overwrites
    /// it within milliseconds, so the row would wait for a result that already
    /// came and went.
    pub pairing_events: broadcast::Sender<oe::Event>,
    /// Most recent battery state (percent, charging) per device UUID.
    pub battery_states: SyncSignal<HashMap<[u8; 6], (u8, bool)>>,
    device_keys: SyncSignal<HashMap<[u8; 6], usize>>,
}

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
/// is a plain value, not a signal guard, so nothing is locked across the awaits.
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

impl HubContext {
    pub(crate) fn new() -> Self {
        let (tracking_events, _) = broadcast::channel(TRACKING_CHANNEL_CAPACITY);
        let (impact_events, _) = broadcast::channel(IMPACT_CHANNEL_CAPACITY);
        let (pairing_events, _) = broadcast::channel(PAIRING_CHANNEL_CAPACITY);
        Self {
            client: SyncSignal::new_maybe_sync(None),
            devices: SyncSignal::new_maybe_sync(Slab::new()),
            dongles: SyncSignal::new_maybe_sync(Vec::new()),
            latest_event: Signal::new(None),
            tracking_events,
            impact_events,
            pairing_events,
            battery_states: SyncSignal::new_maybe_sync(HashMap::new()),
            device_keys: SyncSignal::new_maybe_sync(HashMap::new()),
        }
    }

    /// Follows the hub server for as long as the caller keeps this future
    /// alive, reconnecting whenever a session ends.
    pub(crate) async fn run(&self) {
        let backoff = ConstantBuilder::new()
            .with_delay(RECONNECT_DELAY)
            .without_max_times();
        // A session never ends well, so with no retry limit this only
        // completes by being dropped.
        let _ = (|| {
            let mut hub = self.clone();
            async move { hub.follow_server().await }
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
    async fn follow_server(&mut self) -> Result<Infallible, LinkDown> {
        tracing::info!("Hub connecting to server...");
        let (client, subscriptions) = connect().await.map_err(LinkDown::Connect)?;
        // Publish only once connected, so readers never see a half-dialled
        // client.
        self.client.set(Some(client));
        Err(LinkDown::Session(self.run_session(subscriptions).await))
    }

    /// Applies updates until any stream fails or ends; the stale half of a
    /// broken subscription must not keep serving the UI.
    async fn run_session(
        &mut self,
        subscriptions: Subscriptions<impl Stream<Item = HubUpdate>>,
    ) -> SessionEnd {
        self.replace_devices(subscriptions.initial_devices);

        let mut updates = pin!(subscriptions.updates);
        while let Some(update) = updates.next().await {
            match update {
                HubUpdate::Devices(Ok(list)) => self.apply_device_list(list),
                HubUpdate::Dongles(Ok(list)) => self.apply_dongles(list),
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

    fn replace_devices(&mut self, list: Vec<Device>) {
        let mut devices = self.devices.write();
        let mut keys = self.device_keys.write();
        devices.clear();
        keys.clear();

        for device in list {
            let uuid = device.uuid;
            let idx = devices.insert(device);
            keys.insert(uuid, idx);
        }
    }

    fn apply_device_list(&mut self, list: Vec<Device>) {
        tracing::debug!("Device list update: {} devices", list.len());
        self.replace_devices(list);
    }

    fn apply_dongles(&mut self, list: Vec<DongleInfo>) {
        tracing::debug!("Dongle list update: {} dongles", list.len());
        self.dongles.set(list);
    }

    fn handle_event(&mut self, event: oe::Event) {
        tracing::trace!("Hub event: {:?}", event);
        if let oe::Event::DeviceEvent(oe::DeviceEvent(device, kind)) = &event {
            match kind {
                oe::DeviceEventKind::TrackingEvent(tracking) => {
                    self.broadcast_tracking(device, *tracking);
                }
                oe::DeviceEventKind::ImpactEvent(impact) => {
                    // No subscribers is the normal case outside the zero view.
                    let _ = self.impact_events.send((device.clone(), *impact));
                }
                oe::DeviceEventKind::BatteryEvent(battery) => {
                    self.record_battery(device, battery);
                }
                _ => {}
            }
        }
        if is_pairing_result(&event) {
            let _ = self.pairing_events.send(event.clone());
        }
        self.latest_event.set(Some(event));
    }

    fn broadcast_tracking(&self, device: &Device, tracking: oe::TrackingEvent) {
        tracing::trace!(
            "Hub received TrackingEvent for device {:?}: aimpoint=({}, {}), screen_id={}",
            device.uuid,
            tracking.aimpoint.x,
            tracking.aimpoint.y,
            tracking.screen_id
        );
        match self.tracking_events.send((device.clone(), tracking)) {
            Ok(_) => tracing::debug!(
                "Successfully broadcast tracking event to {} subscribers",
                self.tracking_events.receiver_count()
            ),
            Err(error) => {
                tracing::trace!("Failed to broadcast tracking event: {error} (no subscribers?)");
            }
        }
    }

    fn record_battery(&mut self, device: &Device, battery: &oe::BatteryEvent) {
        tracing::info!(
            "BatteryEvent received for {:02x?}: {}% charging={}",
            device.uuid,
            battery.percent,
            battery.charging
        );
        self.battery_states
            .write()
            .insert(device.uuid, (battery.percent, battery.charging));
    }

    pub(crate) fn device_key(&self, device: &Device) -> Option<usize> {
        self.device_keys.peek().get(&device.uuid).copied()
    }

    /// The connected client, or `None` before the hub server first answers.
    /// Peeks, so the caller is not subscribed to reconnects.
    pub(crate) fn client(&self) -> Option<Client> {
        self.client.peek().clone()
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
