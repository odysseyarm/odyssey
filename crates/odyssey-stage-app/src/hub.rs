//! Bridge to the running hub: the device list, live tracking of physical
//! devices, and the simulator the hub may own.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use odyssey_hub_client::client::Client;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::{DeviceEventKind, Event};
use odyssey_hub_server::sim::Control;
use odyssey_sim::Snapshot;
use serde::Serialize;
use tokio::sync::watch;

const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const SIM_POLL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Tracking {
    pub(crate) aim_uv: [f64; 2],
    /// Stage coordinates: meters from the display's top-left, +Z away from the viewer.
    pub(crate) position_m: [f64; 3],
    pub(crate) distance_m: f64,
    pub(crate) screen_id: u32,
    #[serde(skip)]
    pub(crate) at: Option<Instant>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HubDevice {
    pub(crate) uuid: String,
    pub(crate) name: String,
    pub(crate) simulated: bool,
    pub(crate) product_id: u16,
    pub(crate) firmware: [u16; 3],
    pub(crate) events_connected: bool,
    pub(crate) tracking: Option<Tracking>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct HubView {
    pub(crate) connected: bool,
    pub(crate) error: Option<String>,
    pub(crate) devices: Vec<HubDevice>,
    /// Present when the hub runs the simulator and its control socket answers.
    pub(crate) sim: Option<Snapshot>,
    /// The hub's screen 0, as size in meters and the top-left offset in its frame.
    pub(crate) screen: Option<HubScreen>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct HubScreen {
    pub(crate) width_m: f64,
    pub(crate) height_m: f64,
    pub(crate) origin_m: [f64; 2],
}

pub(crate) fn uuid_hex(uuid: [u8; 6]) -> String {
    hex::encode(uuid)
}

fn device_name(device: &Device) -> String {
    let end = device
        .name
        .iter()
        .position(|b| *b == 0)
        .unwrap_or(device.name.len());
    String::from_utf8_lossy(&device.name[..end]).into_owned()
}

/// The hub's simulator state, over a connection kept between polls.
async fn poll_simulator(control: &mut Option<Control>) -> Option<Snapshot> {
    let mut open = match control.take() {
        Some(open) => open,
        None => Control::connect().await.ok()?,
    };
    let snapshot = open.snapshot().await.ok()?;
    *control = Some(open);
    Some(snapshot)
}

pub(crate) fn spawn(view: watch::Sender<HubView>) {
    tokio::spawn(async move {
        loop {
            let error = match run(&view).await {
                Ok(()) => "the hub closed the connection".to_owned(),
                Err(error) => error,
            };
            view.send_modify(|state| {
                state.connected = false;
                state.error = Some(error);
                state.devices.clear();
                state.sim = None;
            });
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    });
}

async fn run(view: &watch::Sender<HubView>) -> Result<(), String> {
    let client = Client::connect()
        .await
        .map_err(|error| format!("no hub running: {error}"))?;
    let mut devices = Box::pin(
        client
            .subscribe_device_list()
            .await
            .map_err(|error| error.to_string())?,
    );
    let mut events = client
        .subscribe_events_passive()
        .await
        .map_err(|error| error.to_string())?;
    view.send_modify(|state| {
        state.connected = true;
        state.error = None;
    });
    refresh_screen(&client, view).await;
    let mut poll = tokio::time::interval(SIM_POLL);
    let mut simulator = None;
    loop {
        tokio::select! {
            list = devices.next() => {
                let Some(list) = list else { return Ok(()) };
                let list = list.map_err(|error| error.to_string())?;
                apply_list(view, &list);
                refresh_screen(&client, view).await;
            }
            event = events.next() => {
                let Some(event) = event else { return Ok(()) };
                if let Event::DeviceEvent(event) = event.map_err(|error| error.to_string())? {
                    apply_event(view, &event.0, &event.1);
                }
            }
            _ = poll.tick() => {
                let sim = poll_simulator(&mut simulator).await;
                view.send_if_modified(|state| {
                    let changed = state.sim.as_ref().map(|s| s.time_ms) != sim.as_ref().map(|s| s.time_ms);
                    state.sim = sim;
                    changed
                });
            }
        }
    }
}

fn apply_list(view: &watch::Sender<HubView>, list: &[Device]) {
    view.send_modify(|state| {
        let mut previous: BTreeMap<String, Option<Tracking>> = state
            .devices
            .drain(..)
            .map(|device| (device.uuid, device.tracking))
            .collect();
        state.devices = list
            .iter()
            .map(|device| {
                let uuid = uuid_hex(device.uuid);
                HubDevice {
                    tracking: previous.remove(&uuid).flatten(),
                    name: device_name(device),
                    simulated: odyssey_sim::is_simulated(&device.uuid),
                    product_id: device.product_id,
                    firmware: device.firmware_version,
                    events_connected: device.events_connected,
                    uuid,
                }
            })
            .collect();
    });
}

async fn refresh_screen(client: &Client, view: &watch::Sender<HubView>) {
    let screen = client.get_screen_info_by_id(0).await.ok().and_then(|info| {
        let [top_left, top_right, bottom_left, _] = info.bounds;
        let width = f64::from((top_right - top_left).norm());
        let height = f64::from((bottom_left - top_left).norm());
        (width > 0.0 && height > 0.0).then(|| HubScreen {
            width_m: width,
            height_m: height,
            origin_m: [f64::from(top_left.x), f64::from(top_left.y)],
        })
    });
    view.send_modify(|state| state.screen = screen);
}

fn apply_event(view: &watch::Sender<HubView>, device: &Device, kind: &DeviceEventKind) {
    let DeviceEventKind::TrackingEvent(tracking) = kind else {
        return;
    };
    view.send_if_modified(|state| {
        let uuid = uuid_hex(device.uuid);
        let origin = state.screen.map_or([0.0, 0.0], |screen| screen.origin_m);
        let Some(entry) = state.devices.iter_mut().find(|entry| entry.uuid == uuid) else {
            return false;
        };
        // Client coordinates flip Y and Z relative to the scene frame.
        let translation = tracking.pose.translation;
        entry.tracking = Some(Tracking {
            aim_uv: [
                f64::from(tracking.aimpoint.x),
                f64::from(tracking.aimpoint.y),
            ],
            position_m: [
                f64::from(translation[0]) - origin[0],
                -f64::from(translation[1]) - origin[1],
                -f64::from(translation[2]),
            ],
            distance_m: f64::from(tracking.distance),
            screen_id: tracking.screen_id,
            at: Some(Instant::now()),
        });
        false
    });
}
