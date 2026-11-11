mod ota;
mod native;
mod transport;

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;

use btleplug::platform::Peripheral;
use odyssey_hub_common::accessory::{
    AccessoryConnectionInfo, AccessoryConnectionState, AccessoryInfo, AccessoryInfoMap,
    AccessoryMap, AccessoryRadio, AccessoryRadioInfo, AccessoryType, DiscoveredAccessory,
};
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event, ImpactEvent};
use odyssey_protocol::{accessory as wire, marker_control};
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use self::transport::{Link, RadioBackend};
use crate::device_list::DeviceList;
use crate::dongle_list::DongleList;

const MANTIS_SHOT_MARKER: u8 = wire::MANTIS_SHOT_MARKER;
const ACCESSORY_IMPACT_TIMESTAMP: u32 = u32::MAX;
const MARKER_RENEW_INTERVAL: Duration = Duration::from_millis(marker_control::RENEW_MILLIS);
pub(crate) type MarkerDemandCounts = [usize; (odyssey_common::MAX_SCREEN_ID + 1) as usize];

#[derive(Clone)]
pub(crate) struct MarkerDemand(watch::Sender<MarkerDemandCounts>);

impl Default for MarkerDemand {
    fn default() -> Self {
        Self(watch::channel(MarkerDemandCounts::default()).0)
    }
}

impl MarkerDemand {
    pub(crate) fn subscribe(&self) -> watch::Receiver<MarkerDemandCounts> {
        self.0.subscribe()
    }

    pub(crate) fn acquire(&self, screen_id: Option<u8>) -> MarkerDemandGuard {
        self.0.send_modify(|counts| {
            for (screen, count) in counts.iter_mut().enumerate() {
                if screen_id.is_none_or(|id| usize::from(id) == screen) {
                    *count += 1;
                }
            }
        });
        MarkerDemandGuard(self.0.clone(), screen_id)
    }
}

pub(crate) struct MarkerDemandGuard(watch::Sender<MarkerDemandCounts>, Option<u8>);

impl Drop for MarkerDemandGuard {
    fn drop(&mut self) {
        self.0.send_modify(|counts| {
            for (screen, count) in counts.iter_mut().enumerate() {
                if self.1.is_none_or(|id| usize::from(id) == screen) {
                    *count -= 1;
                }
            }
        });
    }
}

#[derive(Debug)]
enum OneShotAction {
    Enroll,
    ResetMarkerMode,
    MarkerOta {
        desktop_version: semver::Version,
        result: oneshot::Sender<Result<String, String>>,
    },
}

/// Why a marker mode reset was refused before touching any radio.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ResetMarkerModeError {
    #[error("accessory is not configured")]
    NotConfigured,
    #[error("accessory is not a marker control board")]
    NotMarker,
    #[error("{0}")]
    Failed(String),
}

pub(crate) struct OneShotCommand {
    address: [u8; 6],
    info: AccessoryInfo,
    action: OneShotAction,
    reply: oneshot::Sender<Result<AccessoryRadio, String>>,
}

#[derive(Clone)]
pub(crate) struct AccessoryRadios {
    pub(crate) snapshots: watch::Sender<Vec<AccessoryRadioInfo>>,
    commands: mpsc::Sender<OneShotCommand>,
}

impl AccessoryRadios {
    pub(crate) fn new() -> (Self, mpsc::Receiver<OneShotCommand>) {
        let (commands, receiver) = mpsc::channel(16);
        (
            Self {
                snapshots: watch::channel(Vec::new()).0,
                commands,
            },
            receiver,
        )
    }

    pub(crate) async fn enroll(
        &self,
        address: [u8; 6],
        info: AccessoryInfo,
    ) -> Result<AccessoryRadio, String> {
        self.one_shot(address, info, OneShotAction::Enroll).await
    }

    /// Connects to the saved radio, writes Mode=Auto and disconnects.
    pub(crate) async fn reset_marker_mode(
        &self,
        address: [u8; 6],
        configured: &AccessoryInfoMap,
    ) -> Result<(), ResetMarkerModeError> {
        let info = configured
            .get(&address)
            .ok_or(ResetMarkerModeError::NotConfigured)?;
        if info.ty != AccessoryType::MarkerControlBoard {
            return Err(ResetMarkerModeError::NotMarker);
        }
        self.one_shot(address, info.clone(), OneShotAction::ResetMarkerMode)
            .await
            .map(|_| ())
            .map_err(ResetMarkerModeError::Failed)
    }

    pub(crate) async fn update_marker_ota(
        &self,
        address: [u8; 6],
        configured: &AccessoryInfoMap,
        desktop_version: semver::Version,
    ) -> Result<String, String> {
        let info = configured
            .get(&address)
            .ok_or_else(|| "Marker is not configured".to_string())?;
        if info.ty != AccessoryType::MarkerControlBoard {
            return Err("Accessory is not a marker control board".into());
        }
        let (result, response) = oneshot::channel();
        self.one_shot(
            address,
            info.clone(),
            OneShotAction::MarkerOta {
                desktop_version,
                result,
            },
        )
        .await?;
        response.await.map_err(|error| error.to_string())?
    }
    async fn one_shot(
        &self,
        address: [u8; 6],
        info: AccessoryInfo,
        action: OneShotAction,
    ) -> Result<AccessoryRadio, String> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(OneShotCommand {
                address,
                info,
                action,
                reply,
            })
            .await
            .map_err(|e| e.to_string())?;
        response.await.map_err(|e| e.to_string())?
    }
}

#[derive(Clone)]
struct RadioDescriptor {
    radio: AccessoryRadio,
    name: String,
    is_default: bool,
    available: bool,
    backend: RadioBackend,
}

enum RadioEvent {
    Enumerated(Result<Vec<RadioDescriptor>, String>),
    Discovery {
        radio: AccessoryRadio,
        nearby: Vec<DiscoveredAccessory>,
    },
    Capabilities {
        radio: AccessoryRadio,
        capabilities: wire::Capabilities,
    },
    RadioError {
        radio: AccessoryRadio,
        error: String,
    },
    Connection {
        address: [u8; 6],
        generation: u64,
        state: AccessoryConnectionState,
        error: Option<String>,
        physical: bool,
    },
}

struct RadioWorker {
    info: AccessoryRadioInfo,
    backend: RadioBackend,
    nearby: watch::Receiver<HashMap<[u8; 6], Peripheral>>,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

struct Session {
    radio: AccessoryRadio,
    ty: AccessoryType,
    generation: u64,
    stop: CancellationToken,
    task: JoinHandle<()>,
    one_shot: bool,
    info: AccessoryInfo,
}

#[derive(Default)]
struct AccessoryState {
    session: Option<Session>,
    status: Option<ConnectionStatus>,
    pending_one_shot: Option<OneShotCommand>,
}

struct ConnectionStatus {
    state: AccessoryConnectionState,
    error: Option<String>,
    physical: bool,
}

struct AccessoryManager {
    radios: HashMap<AccessoryRadio, RadioWorker>,
    states: HashMap<[u8; 6], AccessoryState>,
    generation: u64,
    info: watch::Receiver<AccessoryInfoMap>,
    demand: watch::Receiver<MarkerDemandCounts>,
    handle: AccessoryRadios,
    commands: mpsc::Receiver<OneShotCommand>,
    events: mpsc::Receiver<RadioEvent>,
    updates: mpsc::Sender<RadioEvent>,
    event_tx: broadcast::Sender<Event>,
    map_tx: broadcast::Sender<AccessoryMap>,
    shared_map: Arc<Mutex<AccessoryMap>>,
    device_list: DeviceList,
    dongle_list: DongleList,
    stop: CancellationToken,
    retired: Vec<JoinHandle<()>>,
    native_error: Option<String>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "one handle per channel/list the manager owns; grouping would only rename them"
)]
pub(crate) async fn accessory_manager(
    event_tx: broadcast::Sender<Event>,
    info: watch::Receiver<AccessoryInfoMap>,
    map_tx: broadcast::Sender<AccessoryMap>,
    device_list: DeviceList,
    shared_map: Arc<Mutex<AccessoryMap>>,
    demand: watch::Receiver<MarkerDemandCounts>,
    dongle_list: DongleList,
    handle: AccessoryRadios,
    commands: mpsc::Receiver<OneShotCommand>,
    native_enabled: bool,
    stop: CancellationToken,
) {
    let (updates, events) = mpsc::channel(256);
    let enumeration_stop = stop.clone();
    let enumeration_updates = updates.clone();
    let enumerator = tokio::spawn(async move {
        let mut poll = time::interval(Duration::from_secs(1));
        let mut native = None;
        loop {
            tokio::select! {
                () = enumeration_stop.cancelled() => break,
                _ = poll.tick() => {}
            }
            let result = if native_enabled {
                if native.is_none() {
                    match native::NativeManager::new().await {
                        Ok(manager) => native = Some(manager),
                        Err(error) => {
                            if enumeration_updates
                                .send(RadioEvent::Enumerated(Err(error)))
                                .await
                                .is_err()
                            {
                                break;
                            }
                            continue;
                        }
                    }
                }
                tokio::select! {
                    () = enumeration_stop.cancelled() => break,
                    result = native.as_ref().expect("initialized native manager").enumerate() => result,
                }
            } else {
                Ok(Vec::new())
            };
            if enumeration_updates
                .send(RadioEvent::Enumerated(result))
                .await
                .is_err()
            {
                break;
            }
        }
    });
    let mut manager = AccessoryManager {
        radios: HashMap::new(),
        states: HashMap::new(),
        generation: 0,
        info,
        demand,
        handle,
        commands,
        events,
        updates,
        event_tx,
        map_tx,
        shared_map,
        device_list,
        dongle_list,
        stop,
        retired: Vec::new(),
        native_error: None,
    };
    manager.run().await;
    enumerator.abort();
    for (_, state) in manager.states.drain() {
        if let Some(session) = state.session {
            session.stop.cancel();
            let _ = session.task.await;
        }
        if let Some(command) = state.pending_one_shot {
            let _ = command.reply.send(Err("Accessory manager stopped".into()));
        }
    }
    for task in manager.retired {
        let _ = task.await;
    }
}

impl AccessoryManager {
    async fn run(&mut self) {
        let mut poll = time::interval(Duration::from_millis(200));
        self.publish();
        loop {
            tokio::select! {
                () = self.stop.cancelled() => break,
                changed = self.info.changed() => if changed.is_err() { break; },
                changed = self.demand.changed() => if changed.is_err() { break; },
                command = self.commands.recv() => if let Some(command) = command {
                    let state = self.states.entry(command.address).or_default();
                    if state.pending_one_shot.is_some()
                        || state.session.as_ref().is_some_and(|session| session.one_shot)
                    {
                        let _ = command.reply.send(Err("Accessory operation already in progress".into()));
                    } else {
                        if let Some(session) = &state.session {
                            session.stop.cancel();
                        }
                        state.pending_one_shot = Some(command);
                    }
                },
                event = self.events.recv() => if let Some(event) = event { self.apply(event); },
                _ = poll.tick() => {}
            }
            self.reconcile();
            self.publish();
        }
    }

    fn apply(&mut self, event: RadioEvent) {
        match event {
            RadioEvent::Enumerated(result) => {
                let mut descriptors = match result {
                    Ok(descriptors) => {
                        self.native_error = None;
                        descriptors
                    }
                    Err(error) => {
                        self.native_error = Some(error.clone());
                        warn!("Native Bluetooth enumeration failed: {error}");
                        for worker in self
                            .radios
                            .values_mut()
                            .filter(|r| !matches!(r.info.radio, AccessoryRadio::Dongle { .. }))
                        {
                            worker.info.error = Some(error.clone());
                            worker.info.available = false;
                        }
                        Vec::new()
                    }
                };
                descriptors.extend(self.dongle_list.snapshot().into_iter().map(|dongle| {
                    RadioDescriptor {
                        radio: AccessoryRadio::Dongle {
                            id: dongle.id.clone(),
                        },
                        name: format!("Odyssey dongle {}", dongle.id),
                        is_default: false,
                        available: true,
                        backend: RadioBackend::Dongle(self.dongle_list.clone(), dongle.id),
                    }
                }));
                let removed: Vec<_> = self
                    .radios
                    .iter()
                    .filter(|(radio, worker)| {
                        !descriptors.iter().any(|descriptor| {
                            &descriptor.radio == *radio
                                && descriptor.available == worker.info.available
                        }) || worker.task.is_finished()
                    })
                    .map(|(radio, _)| radio.clone())
                    .collect();
                for radio in removed {
                    if let Some(worker) = self.radios.remove(&radio) {
                        worker.stop.cancel();
                        self.retired.push(worker.task);
                    }
                }
                for descriptor in descriptors {
                    if let Some(worker) = self.radios.get_mut(&descriptor.radio) {
                        worker.info.is_default = descriptor.is_default;
                        continue;
                    }
                    let stop = self.stop.child_token();
                    let (nearby, receiver) = watch::channel(HashMap::new());
                    let backend = descriptor.backend.clone();
                    let updates = self.updates.clone();
                    let worker_stop = stop.clone();
                    let radio = descriptor.radio.clone();
                    let task = tokio::spawn(async move {
                        match backend {
                            RadioBackend::Native(adapter) => {
                                native::discover(adapter, radio, nearby, updates, worker_stop)
                                    .await;
                            }
                            RadioBackend::Dongle(list, id) => {
                                transport::discover_dongle(list, id, updates, worker_stop).await;
                            }
                        }
                    });
                    let mut info = empty_radio(descriptor.radio.clone(), descriptor.name);
                    info.is_default = descriptor.is_default;
                    info.available = descriptor.available;
                    info.accessory_supported =
                        matches!(descriptor.backend, RadioBackend::Native(_));
                    self.radios.insert(
                        descriptor.radio,
                        RadioWorker {
                            info,
                            backend: descriptor.backend,
                            nearby: receiver,
                            stop,
                            task,
                        },
                    );
                }
            }
            RadioEvent::Discovery { radio, mut nearby } => {
                if let Some(worker) = self.radios.get_mut(&radio) {
                    nearby.sort_by_key(|item| item.address);
                    worker.info.nearby = nearby;
                }
            }
            RadioEvent::Capabilities {
                radio,
                capabilities,
            } => {
                if let Some(worker) = self.radios.get_mut(&radio) {
                    worker.info.accessory_supported = true;
                    worker.info.capacity = Some(u32::from(capabilities.total_capacity));
                    worker.info.accessory_capacity =
                        Some(u32::from(capabilities.accessory_capacity));
                    worker.info.connected_accessories =
                        u32::from(capabilities.connected_accessories);
                    worker.info.connecting_accessories =
                        u32::from(capabilities.connecting_accessories);
                    worker.info.connected_trackers = u32::from(capabilities.connected_trackers);
                    worker.info.error = None;
                }
            }
            RadioEvent::RadioError { radio, error } => {
                if let Some(worker) = self.radios.get_mut(&radio) {
                    worker.info.error = Some(error);
                }
            }
            RadioEvent::Connection {
                address,
                generation,
                state: connection_state,
                error,
                physical,
            } => {
                if let Some(accessory) = self.states.get_mut(&address) {
                    if accessory
                        .session
                        .as_ref()
                        .is_some_and(|session| session.generation == generation)
                    {
                        accessory.status = Some(ConnectionStatus {
                            state: connection_state,
                            error,
                            physical,
                        });
                    }
                }
            }
        }
    }

    fn route(&self, radio: &AccessoryRadio) -> Option<AccessoryRadio> {
        match radio {
            AccessoryRadio::System => self
                .radios
                .values()
                .find(|worker| worker.info.is_default)
                .map(|worker| worker.info.radio.clone()),
            _ => Some(radio.clone()),
        }
    }

    fn reconcile(&mut self) {
        let info = self.info.borrow().clone();
        self.retired.retain(|task| !task.is_finished());
        self.states.retain(|address, state| {
            if state
                .session
                .as_ref()
                .is_some_and(|session| session.task.is_finished())
            {
                state.session = None;
                state.status = None;
            }
            if let Some(session) = state.session.as_ref() {
                let same = session.one_shot
                    || info.get(address).is_some_and(|info| {
                        info.ty == session.ty
                            && match &info.radio {
                                AccessoryRadio::System => self
                                    .radios
                                    .get(&session.radio)
                                    .is_some_and(|radio| radio.info.is_default),
                                radio => radio == &session.radio,
                            }
                    });
                if !same
                    || !self
                        .radios
                        .get(&session.radio)
                        .is_some_and(|radio| radio.info.available)
                    || state.pending_one_shot.is_some()
                {
                    session.stop.cancel();
                }
            }
            state.session.is_some() || state.pending_one_shot.is_some()
        });
        let ready: Vec<_> = self
            .states
            .iter()
            .filter_map(|(&address, state)| {
                (state.session.is_none() && state.pending_one_shot.is_some()).then_some(address)
            })
            .collect();
        for address in ready {
            let Some(command) = self
                .states
                .get_mut(&address)
                .and_then(|state| state.pending_one_shot.take())
            else {
                continue;
            };
            self.spawn(
                address,
                command.info.clone(),
                Some((command.action, command.reply)),
            );
        }
        for (address, info) in info {
            let active = self
                .states
                .get(&address)
                .is_some_and(|state| state.session.is_some() || state.pending_one_shot.is_some());
            if !active {
                self.spawn(address, info, None);
            }
        }
    }

    fn spawn(
        &mut self,
        address: [u8; 6],
        info: AccessoryInfo,
        one_shot: Option<(
            OneShotAction,
            oneshot::Sender<Result<AccessoryRadio, String>>,
        )>,
    ) {
        let route = self.route(&info.radio);
        let worker = route
            .as_ref()
            .and_then(|radio| self.radios.get(radio))
            .filter(|worker| worker.info.available && worker.info.accessory_supported);
        let Some(worker) = worker else {
            if let Some((_, reply)) = one_shot {
                let error = route
                    .as_ref()
                    .and_then(|radio| self.radios.get(radio))
                    .and_then(|worker| worker.info.error.clone())
                    .unwrap_or_else(|| {
                        "Selected accessory radio unavailable or accessory capability not supported"
                            .into()
                    });
                let _ = reply.send(Err(error));
            }
            self.states.remove(&address);
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        let one_shot_active = one_shot.is_some();
        let stop = self.stop.child_token();
        let context = SessionContext {
            address,
            generation,
            info: info.clone(),
            backend: worker.backend.clone(),
            nearby: worker.nearby.clone(),
            info_watch: self.info.clone(),
            demand: self.demand.clone(),
            updates: self.updates.clone(),
            event_tx: self.event_tx.clone(),
            device_list: self.device_list.clone(),
            stop: stop.clone(),
        };
        let enrolled_radio = worker.info.radio.clone();
        let task = tokio::spawn(async move {
            if let Some((action, reply)) = one_shot {
                let result = match action {
                    OneShotAction::Enroll => context.enroll().await,
                    OneShotAction::ResetMarkerMode => context.reset_marker_mode().await,
                    OneShotAction::MarkerOta {
                        desktop_version,
                        result,
                    } => {
                        let update = context.marker_ota_update(&desktop_version).await;
                        let outer = update.as_ref().map(|_| ()).map_err(Clone::clone);
                        let _ = result.send(update);
                        outer
                    }
                };
                tracing::info!(?address, ?result, "Accessory one-shot finished");
                let _ = reply.send(result.map(|()| enrolled_radio));
            } else {
                context.run().await;
            }
        });
        let state = self.states.entry(address).or_default();
        state.status = None;
        state.session = Some(Session {
            radio: worker.info.radio.clone(),
            ty: info.ty,
            generation,
            stop,
            task,
            one_shot: one_shot_active,
            info,
        });
    }

    fn publish(&self) {
        let mut configured = self.info.borrow().clone();
        for (&address, state) in &self.states {
            if let Some(session) = &state.session {
                if session.one_shot {
                    configured.insert(address, session.info.clone());
                }
            }
        }
        let mut radios: Vec<_> = self
            .radios
            .values()
            .map(|worker| worker.info.clone())
            .collect();
        if !radios.iter().any(|radio| radio.is_default) {
            let mut system = empty_radio(
                AccessoryRadio::System,
                "System Bluetooth (unavailable)".into(),
            );
            system.error.clone_from(&self.native_error);
            radios.push(system);
        }
        for radio in &mut radios {
            radio.connections.clear();
            if !matches!(radio.radio, AccessoryRadio::Dongle { .. }) {
                radio.connected_accessories = 0;
                radio.connecting_accessories = 0;
            }
        }
        let mut map = AccessoryMap::new();
        for (&address, info) in &configured {
            let route = self
                .route(&info.radio)
                .unwrap_or_else(|| info.radio.clone());
            let index = radios
                .iter()
                .position(|radio| radio.radio == route)
                .unwrap_or_else(|| {
                    radios.push(empty_radio(
                        route.clone(),
                        match &route {
                            AccessoryRadio::System => "System Bluetooth (unavailable)".to_string(),
                            AccessoryRadio::Native { id } => {
                                format!("Bluetooth adapter {id} (unavailable)")
                            }
                            AccessoryRadio::Dongle { id } => {
                                format!("Odyssey dongle {id} (unavailable)")
                            }
                        },
                    ));
                    radios.len() - 1
                });
            let radio = &mut radios[index];
            if !radio.available && !matches!(route, AccessoryRadio::Dongle { .. }) {
                radio.error.clone_from(&self.native_error);
            }
            let status = self.states.get(&address).and_then(|accessory| {
                accessory
                    .session
                    .as_ref()
                    .is_some_and(|session| session.radio == route)
                    .then_some(accessory.status.as_ref())
                    .flatten()
            });
            let (state, error, physical) = if !radio.available || !radio.accessory_supported {
                (AccessoryConnectionState::RadioUnavailable, radio.error.clone().or_else(|| Some("Selected accessory radio unavailable or accessory capability not supported".into())), false)
            } else if let Some(status) = status {
                (status.state, status.error.clone(), status.physical)
            } else {
                (AccessoryConnectionState::Idle, None, false)
            };
            if !matches!(radio.radio, AccessoryRadio::Dongle { .. }) {
                if state == AccessoryConnectionState::Connected {
                    radio.connected_accessories += 1;
                }
                if state == AccessoryConnectionState::Connecting && physical {
                    radio.connecting_accessories += 1;
                }
            }
            radio.connections.push(AccessoryConnectionInfo {
                address,
                state,
                error,
            });
            if let Some(persisted) = self.info.borrow().get(&address) {
                let connected = state == AccessoryConnectionState::Connected
                    && self.route(&persisted.radio).as_ref() == Some(&route)
                    && persisted.ty == info.ty;
                map.insert(address, (persisted.clone(), connected));
            }
        }
        for radio in &mut radios {
            radio
                .connections
                .sort_by_key(|connection| connection.address);
        }
        radios.sort_by_key(|radio| format!("{:?}", radio.radio));
        self.handle.snapshots.send_if_modified(|previous| {
            if *previous == radios {
                false
            } else {
                *previous = radios;
                true
            }
        });
        let mut previous = self.shared_map.lock();
        if *previous != map {
            previous.clone_from(&map);
            let _ = self.map_tx.send(map);
        }
    }
}

fn empty_radio(radio: AccessoryRadio, name: String) -> AccessoryRadioInfo {
    AccessoryRadioInfo {
        is_default: matches!(radio, AccessoryRadio::System),
        radio,
        name,
        available: false,
        accessory_supported: false,
        connected_accessories: 0,
        connecting_accessories: 0,
        connected_trackers: 0,
        capacity: None,
        accessory_capacity: None,
        connections: Vec::new(),
        nearby: Vec::new(),
        error: None,
    }
}

struct SessionContext {
    address: [u8; 6],
    generation: u64,
    info: AccessoryInfo,
    backend: RadioBackend,
    nearby: watch::Receiver<HashMap<[u8; 6], Peripheral>>,
    info_watch: watch::Receiver<AccessoryInfoMap>,
    demand: watch::Receiver<MarkerDemandCounts>,
    updates: mpsc::Sender<RadioEvent>,
    event_tx: broadcast::Sender<Event>,
    device_list: DeviceList,
    stop: CancellationToken,
}

impl SessionContext {
    async fn state(&self, state: AccessoryConnectionState, error: Option<String>, physical: bool) {
        let _ = self
            .updates
            .send(RadioEvent::Connection {
                address: self.address,
                generation: self.generation,
                state,
                error,
                physical,
            })
            .await;
    }

    async fn marker_ota_update(
        mut self,
        desktop_version: &semver::Version,
    ) -> Result<String, String> {
        let mut link = tokio::select! {
            () = self.stop.cancelled() => return Err("Marker OTA canceled".into()),
            link = self.backend.link(self.address, &mut self.nearby) => link?,
        };
        self.state(AccessoryConnectionState::Connecting, None, true)
            .await;
        if let Err(error) = link.connect(&self.info, false).await {
            let _ = link.disconnect(false).await;
            return Err(format!(
                "Could not connect to marker over selected Bluetooth radio: {error}"
            ));
        }
        let result = ota::update(&mut link, &self.info, desktop_version).await;
        let disconnected = link.disconnect(false).await;
        match (result, disconnected) {
            (Ok(message), Ok(())) => Ok(message),
            (Err(error), _) => Err(error),
            (_, Err(error)) => Err(format!(
                "Marker update finished but disconnect failed: {error}"
            )),
        }
    }
    async fn enroll(mut self) -> Result<(), String> {
        let mut link = tokio::select! {
            () = self.stop.cancelled() => return Err("Accessory enrollment canceled".into()),
            link = self.backend.link(self.address, &mut self.nearby) => link?,
        };
        self.state(AccessoryConnectionState::Connecting, None, true)
            .await;
        let result = tokio::select! {
            () = self.stop.cancelled() => Err("Accessory enrollment canceled".into()),
            result = link.connect(&self.info, true) => result,
        };
        let disconnected = link
            .disconnect(self.info.ty == AccessoryType::MarkerControlBoard)
            .await;
        result.and(disconnected)
    }

    async fn reset_marker_mode(mut self) -> Result<(), String> {
        let mut link = tokio::select! {
            () = self.stop.cancelled() => return Err("Marker mode reset canceled".into()),
            link = self.backend.link(self.address, &mut self.nearby) => link?,
        };
        self.state(AccessoryConnectionState::Connecting, None, true)
            .await;
        let mut connected = false;
        let result = tokio::select! {
            () = self.stop.cancelled() => Err("Marker mode reset canceled".into()),
            result = async {
                link.connect(&self.info, false).await?;
                connected = true;
                link.marker_mode(marker_control::Mode::Auto).await
            } => result,
        };
        let disconnected = link.disconnect(connected).await;
        result.and(disconnected)
    }

    async fn run(mut self) {
        loop {
            if self.stop.is_cancelled() {
                return;
            }
            let marker = self.info.ty == AccessoryType::MarkerControlBoard;
            if marker
                && !marker_active(
                    self.address,
                    &self.info_watch.borrow(),
                    &self.demand.borrow(),
                )
            {
                self.state(AccessoryConnectionState::Idle, None, false)
                    .await;
                tokio::select! {
                    () = self.stop.cancelled() => return,
                    _ = self.info_watch.changed() => {},
                    _ = self.demand.changed() => {},
                }
                continue;
            }
            self.state(AccessoryConnectionState::Connecting, None, false)
                .await;
            let link = tokio::select! {
                () = self.stop.cancelled() => return,
                _ = self.demand.changed(), if marker => continue,
                link = self.backend.link(self.address, &mut self.nearby) => link,
            };
            let result = match link {
                Ok(mut link) => {
                    self.state(AccessoryConnectionState::Connecting, None, true)
                        .await;
                    let setup = tokio::select! {
                        () = self.stop.cancelled() => Ok(false),
                        _ = self.demand.changed(), if marker => Ok(false),
                        result = link.connect(&self.info, false) => result.map(|()| true),
                    };
                    let ready = matches!(setup, Ok(true));
                    let result = match setup {
                        Ok(true)
                            if !self.stop.is_cancelled()
                                && (!marker
                                    || marker_active(
                                        self.address,
                                        &self.info_watch.borrow(),
                                        &self.demand.borrow(),
                                    )) =>
                        {
                            self.connected(&mut link).await
                        }
                        Ok(_) => Ok(()),
                        Err(error) => Err(error),
                    };
                    let disconnected = link.disconnect(marker && ready).await;
                    result.and(disconnected)
                }
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                tracing::warn!(?self.address, %error, "Accessory session failed");
                self.state(AccessoryConnectionState::Failed, Some(error), false)
                    .await;
            } else {
                self.state(AccessoryConnectionState::Idle, None, false)
                    .await;
            }
            tokio::select! {
                () = self.stop.cancelled() => return,
                _ = self.info_watch.changed() => {},
                _ = self.demand.changed() => {},
                () = time::sleep(Duration::from_secs(2)) => {}
            }
        }
    }

    async fn connected(&mut self, link: &mut Link) -> Result<(), String> {
        let marker = self.info.ty == AccessoryType::MarkerControlBoard;
        if marker {
            link.marker_demand(true).await?;
        }
        self.state(AccessoryConnectionState::Connected, None, true)
            .await;
        let mut renew = time::interval(MARKER_RENEW_INTERVAL);
        renew.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        renew.tick().await;
        let forwarder = ShotForwarder {
            device_list: self.device_list.clone(),
            event_tx: self.event_tx.clone(),
            info_watch: self.info_watch.clone(),
            id: self.address,
            ty: self.info.ty,
            radio: self.info.radio.clone(),
        };
        loop {
            tokio::select! {
                () = self.stop.cancelled() => return Ok(()),
                _ = self.demand.changed(), if marker => {
                    if !marker_active(self.address, &self.info_watch.borrow(), &self.demand.borrow()) { return Ok(()); }
                }
                _ = self.info_watch.changed() => {
                    if marker && !marker_active(self.address, &self.info_watch.borrow(), &self.demand.borrow()) { return Ok(()); }
                }
                _ = renew.tick(), if marker => link.marker_demand(true).await?,
                notification = link.notification(self.info.ty), if !marker => {
                    if forwarder.is_shot(&notification?) { forwarder.forward_impact(); }
                }
            }
        }
    }
}

async fn io<T>(future: impl Future<Output = Result<T, impl Display>>) -> Result<T, String> {
    time::timeout(Duration::from_secs(8), future)
        .await
        .map_err(|_elapsed| "Bluetooth operation timed out".to_string())?
        .map_err(|error| error.to_string())
}

fn detected_type(
    name: &str,
    services: &[Uuid],
    service_data: &HashMap<Uuid, Vec<u8>>,
) -> Option<AccessoryType> {
    if services.contains(&Uuid::from_bytes(marker_control::SERVICE_UUID))
        || service_data.contains_key(&Uuid::from_bytes(marker_control::SERVICE_UUID))
        || name.starts_with("Odyssey Marker")
    {
        Some(AccessoryType::MarkerControlBoard)
    } else if name.contains("Mantis")
        || services.contains(&Uuid::from_bytes(wire::MANTIS_SERVICE_UUID))
    {
        Some(AccessoryType::MantisX)
    } else if name.contains("DryFireMag")
        || services.contains(&Uuid::from_bytes(wire::NUS_SERVICE_UUID))
    {
        Some(AccessoryType::DryFireMag)
    } else {
        None
    }
}

fn marker_active(id: [u8; 6], info: &AccessoryInfoMap, demand: &MarkerDemandCounts) -> bool {
    info.get(&id)
        .filter(|info| info.ty == AccessoryType::MarkerControlBoard)
        .and_then(|info| info.screen_id)
        .and_then(|screen| demand.get(usize::from(screen)))
        .is_some_and(|count| *count > 0)
}

/// Turns one accessory's notifications into impact events on the device it is assigned to.
struct ShotForwarder {
    device_list: DeviceList,
    event_tx: broadcast::Sender<Event>,
    info_watch: watch::Receiver<AccessoryInfoMap>,
    id: [u8; 6],
    ty: AccessoryType,
    radio: AccessoryRadio,
}

impl ShotForwarder {
    fn is_shot(&self, value: &[u8]) -> bool {
        match self.ty {
            AccessoryType::DryFireMag => true,
            AccessoryType::MantisX => value == [MANTIS_SHOT_MARKER],
            AccessoryType::MarkerControlBoard => false,
        }
    }

    /// The device the accessory is assigned to hears the shot as an impact.
    fn forward_impact(&self) {
        let assignment = self
            .info_watch
            .borrow()
            .get(&self.id)
            .filter(|info| info.ty == self.ty && info.radio == self.radio)
            .and_then(|info| info.assignment);
        let Some(assignment) = assignment else {
            return;
        };
        let assignment_bytes = assignment.get().to_le_bytes();
        // The assignment is a device uuid in the low 6 bytes.
        let Some(&device_uuid) = assignment_bytes.first_chunk::<6>() else {
            return;
        };
        let Some(device) = self.device_list.find(device_uuid) else {
            return;
        };
        let impact = DeviceEventKind::ImpactEvent(ImpactEvent {
            timestamp: ACCESSORY_IMPACT_TIMESTAMP,
        });
        if let Err(e) = self
            .event_tx
            .send(Event::DeviceEvent(DeviceEvent(device, impact)))
        {
            warn!("accessory_manager: failed to forward impact event: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};
    use odyssey_hub_common::firmware_version::FirmwareVersion;

    use super::*;
    fn manager_for_state_test(
        info: AccessoryInfoMap,
    ) -> (
        AccessoryManager,
        watch::Sender<AccessoryInfoMap>,
        MarkerDemand,
    ) {
        let (info_sender, info) = watch::channel(info);
        let demand = MarkerDemand::default();
        let (handle, commands) = AccessoryRadios::new();
        let (updates, events) = mpsc::channel(64);
        (
            AccessoryManager {
                radios: HashMap::new(),
                states: HashMap::new(),
                generation: 0,
                info,
                demand: demand.subscribe(),
                handle,
                commands,
                events,
                updates,
                event_tx: broadcast::channel(4).0,
                map_tx: broadcast::channel(4).0,
                shared_map: Arc::default(),
                device_list: DeviceList::default(),
                dongle_list: DongleList::default(),
                stop: CancellationToken::new(),
                retired: Vec::new(),
                native_error: None,
            },
            info_sender,
            demand,
        )
    }

    fn insert_state_radio(
        manager: &mut AccessoryManager,
        id: &str,
        default: bool,
    ) -> AccessoryRadio {
        let radio = AccessoryRadio::Native { id: id.into() };
        let mut info = empty_radio(radio.clone(), id.into());
        info.available = true;
        info.accessory_supported = true;
        info.is_default = default;
        let stop = manager.stop.child_token();
        manager.radios.insert(
            radio.clone(),
            RadioWorker {
                info,
                backend: RadioBackend::Dongle(DongleList::default(), id.into()),
                nearby: watch::channel(HashMap::new()).1,
                task: tokio::spawn(stop.clone().cancelled_owned()),
                stop,
            },
        );
        radio
    }

    fn insert_state_session(
        manager: &mut AccessoryManager,
        address: [u8; 6],
        info: AccessoryInfo,
        radio: AccessoryRadio,
        state: AccessoryConnectionState,
    ) {
        manager.generation += 1;
        let generation = manager.generation;
        let stop = manager.stop.child_token();
        manager.states.entry(address).or_default().session = Some(Session {
            radio,
            ty: info.ty,
            generation,
            task: tokio::spawn(stop.clone().cancelled_owned()),
            stop,
            one_shot: false,
            info,
        });
        manager.apply(RadioEvent::Connection {
            address,
            generation,
            state,
            error: None,
            physical: true,
        });
    }

    #[tokio::test]
    async fn native_snapshots_count_live_and_pending_separately_without_seven_link_limit() {
        let (mut manager, info_sender, _demand) = manager_for_state_test(HashMap::new());
        let radio = insert_state_radio(&mut manager, "00:11:22:33:44:55", true);
        let mut configured = HashMap::new();
        for n in 0..10 {
            let mut info = AccessoryInfo::with_defaults("mag", AccessoryType::DryFireMag);
            info.radio = radio.clone();
            configured.insert([n; 6], info.clone());
            insert_state_session(
                &mut manager,
                [n; 6],
                info,
                radio.clone(),
                if n < 8 {
                    AccessoryConnectionState::Connected
                } else {
                    AccessoryConnectionState::Connecting
                },
            );
        }
        info_sender.send_replace(configured);
        manager.publish();
        let snapshots = manager.handle.snapshots.borrow();
        let native = snapshots.iter().find(|info| info.radio == radio).unwrap();
        assert_eq!(
            (native.connected_accessories, native.connecting_accessories),
            (8, 2)
        );
        assert_eq!((native.capacity, native.accessory_capacity), (None, None));
        assert_eq!(
            manager
                .shared_map
                .lock()
                .values()
                .filter(|(_, connected)| *connected)
                .count(),
            8
        );
        manager.stop.cancel();
    }

    #[tokio::test]
    async fn duplicate_observations_have_one_owner_and_unplug_never_uses_default_radio() {
        let (mut manager, info_sender, _demand) = manager_for_state_test(HashMap::new());
        let first = insert_state_radio(&mut manager, "first", true);
        let assigned = insert_state_radio(&mut manager, "assigned", false);
        let seen = DiscoveredAccessory {
            address: [9; 6],
            name: "mag".into(),
            ty: AccessoryType::DryFireMag,
            pairing_window_expires_at_ms: None,
        };
        manager.apply(RadioEvent::Discovery {
            radio: first.clone(),
            nearby: vec![seen.clone()],
        });
        manager.apply(RadioEvent::Discovery {
            radio: assigned.clone(),
            nearby: vec![seen],
        });
        let mut info = AccessoryInfo::with_defaults("mag", AccessoryType::DryFireMag);
        info.radio = assigned.clone();
        info_sender.send_replace(HashMap::from([([9; 6], info.clone())]));
        insert_state_session(
            &mut manager,
            [9; 6],
            info.clone(),
            assigned.clone(),
            AccessoryConnectionState::Connected,
        );
        manager.publish();
        let snapshots = manager.handle.snapshots.borrow();
        assert_eq!(
            snapshots
                .iter()
                .find(|info| info.radio == first)
                .unwrap()
                .connected_accessories,
            0
        );
        assert_eq!(
            snapshots
                .iter()
                .find(|info| info.radio == assigned)
                .unwrap()
                .connected_accessories,
            1
        );
        assert_eq!(
            snapshots
                .iter()
                .filter(|radio| radio.nearby.iter().any(|item| item.address == [9; 6]))
                .count(),
            2
        );
        drop(snapshots);
        manager.radios.remove(&assigned).unwrap().stop.cancel();
        manager.reconcile();
        manager.publish();
        let snapshots = manager.handle.snapshots.borrow();
        let missing = snapshots
            .iter()
            .find(|radio| radio.radio == assigned)
            .unwrap();
        assert!(!missing.available);
        assert_eq!(
            missing.connections[0].state,
            AccessoryConnectionState::RadioUnavailable
        );
        assert_eq!(manager.shared_map.lock().get(&[9; 6]), Some(&(info, false)));
        let session = manager.states[&[9; 6]].session.as_ref().unwrap();
        assert!(session.stop.is_cancelled());
        assert_eq!(session.radio, assigned);
        manager.stop.cancel();
    }

    #[tokio::test]
    async fn marker_idle_uses_no_radio_and_demand_wakes_then_acknowledges_off_before_disconnect() {
        use crate::device_tasks::{DongleInfo, DongleTaskMessage};
        let mut info = AccessoryInfo::with_defaults("markers", AccessoryType::MarkerControlBoard);
        info.radio = AccessoryRadio::Dongle { id: "test".into() };
        info.screen_id = Some(odyssey_common::MAX_SCREEN_ID);
        let (_info_sender, info_watch) = watch::channel(HashMap::from([([1; 6], info.clone())]));
        let demand = MarkerDemand::default();
        let list = DongleList::default();
        let (commands, mut incoming) = mpsc::channel(8);
        list.upsert(
            DongleInfo {
                id: "test".into(),
                protocol_version: [0; 3],
                control_protocol_version: [0; 3],
                firmware_version: [0; 3],
                connected_devices: Vec::new(),
                bonded_devices: Vec::new(),
            },
            commands,
        );
        let (updates, mut states) = mpsc::channel(16);
        let stop = CancellationToken::new();
        let context = SessionContext {
            address: [1; 6],
            generation: 1,
            info,
            backend: RadioBackend::Dongle(list, "test".into()),
            nearby: watch::channel(HashMap::new()).1,
            info_watch,
            demand: demand.subscribe(),
            updates,
            event_tx: broadcast::channel(4).0,
            device_list: DeviceList::default(),
            stop: stop.clone(),
        };
        let task = tokio::spawn(context.run());
        let state = states.recv().await.unwrap();
        assert!(matches!(
            state,
            RadioEvent::Connection {
                state: AccessoryConnectionState::Idle,
                physical: false,
                ..
            }
        ));
        assert!(incoming.try_recv().is_err());
        let guard = demand.acquire(Some(odyssey_common::MAX_SCREEN_ID));
        let events = broadcast::channel(4).0;
        let subscription = time::timeout(Duration::from_millis(500), incoming.recv())
            .await
            .unwrap()
            .unwrap();
        match subscription {
            DongleTaskMessage::SubscribeAccessoryEvents { reply } => {
                reply.send(Ok(events.subscribe())).unwrap();
            }
            _ => panic!("expected notification subscription"),
        }
        for expected in [
            wire::Command::Connect {
                address: [1; 6],
                profile: wire::Profile::MarkerControlBoard,
                enroll: false,
            },
            wire::Command::MarkerDemand {
                address: [1; 6],
                active: false,
            },
            wire::Command::MarkerDemand {
                address: [1; 6],
                active: true,
            },
        ] {
            let message = time::timeout(Duration::from_millis(500), incoming.recv())
                .await
                .unwrap()
                .unwrap();
            match message {
                DongleTaskMessage::Accessory { command, reply } => {
                    assert_eq!(command, expected);
                    reply.send(Ok(wire::Reply::Done())).unwrap();
                }
                _ => panic!("expected accessory command"),
            }
        }
        drop(guard);
        for expected in [
            wire::Command::MarkerDemand {
                address: [1; 6],
                active: false,
            },
            wire::Command::Disconnect { address: [1; 6] },
        ] {
            let message = time::timeout(Duration::from_millis(500), incoming.recv())
                .await
                .unwrap()
                .unwrap();
            match message {
                DongleTaskMessage::Accessory { command, reply } => {
                    assert_eq!(command, expected);
                    reply.send(Ok(wire::Reply::Done())).unwrap();
                }
                _ => panic!("expected accessory command"),
            }
        }
        stop.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn reset_marker_mode_connects_writes_auto_acknowledged_then_disconnects() {
        use crate::device_tasks::{DongleInfo, DongleTaskMessage};
        let mut info = AccessoryInfo::with_defaults("markers", AccessoryType::MarkerControlBoard);
        info.radio = AccessoryRadio::Dongle { id: "test".into() };
        let list = DongleList::default();
        let (commands, mut incoming) = mpsc::channel(8);
        list.upsert(
            DongleInfo {
                id: "test".into(),
                protocol_version: [0; 3],
                control_protocol_version: [0; 3],
                firmware_version: [0; 3],
                connected_devices: Vec::new(),
                bonded_devices: Vec::new(),
            },
            commands,
        );
        let context = SessionContext {
            address: [1; 6],
            generation: 1,
            info: info.clone(),
            backend: RadioBackend::Dongle(list, "test".into()),
            nearby: watch::channel(HashMap::new()).1,
            info_watch: watch::channel(HashMap::from([([1; 6], info)])).1,
            demand: MarkerDemand::default().subscribe(),
            updates: mpsc::channel(16).0,
            event_tx: broadcast::channel(4).0,
            device_list: DeviceList::default(),
            stop: CancellationToken::new(),
        };
        let task = tokio::spawn(context.reset_marker_mode());
        let events = broadcast::channel(4).0;
        match time::timeout(Duration::from_millis(500), incoming.recv())
            .await
            .unwrap()
            .unwrap()
        {
            DongleTaskMessage::SubscribeAccessoryEvents { reply } => {
                reply.send(Ok(events.subscribe())).unwrap();
            }
            _ => panic!("expected notification subscription"),
        }
        for expected in [
            wire::Command::Connect {
                address: [1; 6],
                profile: wire::Profile::MarkerControlBoard,
                enroll: false,
            },
            wire::Command::MarkerDemand {
                address: [1; 6],
                active: false,
            },
            wire::Command::MarkerMode {
                address: [1; 6],
                mode: marker_control::Mode::Auto,
            },
            wire::Command::MarkerDemand {
                address: [1; 6],
                active: false,
            },
            wire::Command::Disconnect { address: [1; 6] },
        ] {
            match time::timeout(Duration::from_millis(500), incoming.recv())
                .await
                .unwrap()
                .unwrap()
            {
                DongleTaskMessage::Accessory { command, reply } => {
                    assert_eq!(command, expected);
                    reply.send(Ok(wire::Reply::Done())).unwrap();
                }
                _ => panic!("expected accessory command"),
            }
        }
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn reset_marker_mode_rejects_non_marker_and_unknown_without_queueing_radio_work() {
        let (radios, mut commands) = AccessoryRadios::new();
        let configured = AccessoryInfoMap::from([
            (
                [1; 6],
                AccessoryInfo::with_defaults("mag", AccessoryType::DryFireMag),
            ),
            (
                [2; 6],
                AccessoryInfo::with_defaults("markers", AccessoryType::MarkerControlBoard),
            ),
        ]);
        assert_eq!(
            radios.reset_marker_mode([1; 6], &configured).await,
            Err(ResetMarkerModeError::NotMarker)
        );
        assert_eq!(
            radios.reset_marker_mode([3; 6], &configured).await,
            Err(ResetMarkerModeError::NotConfigured)
        );
        assert!(commands.try_recv().is_err());
        drop(commands);
        assert!(matches!(
            radios.reset_marker_mode([2; 6], &configured).await,
            Err(ResetMarkerModeError::Failed(_))
        ));
    }

    fn forwarder(ty: AccessoryType) -> ShotForwarder {
        let (event_tx, _) = broadcast::channel(4);
        let (_, info_watch) = watch::channel(AccessoryInfoMap::new());
        ShotForwarder {
            device_list: DeviceList::default(),
            event_tx,
            info_watch,
            id: [0; 6],
            ty,
            radio: AccessoryRadio::System,
        }
    }

    #[test]
    fn dry_fire_mag_notifies_on_every_shot_and_mantis_only_on_its_marker() {
        let dry_fire = forwarder(AccessoryType::DryFireMag);
        assert!(dry_fire.is_shot(&[]));
        assert!(dry_fire.is_shot(&[7, 7]));

        let mantis = forwarder(AccessoryType::MantisX);
        assert!(mantis.is_shot(&[MANTIS_SHOT_MARKER]));
        assert!(!mantis.is_shot(&[0]));
        assert!(!mantis.is_shot(&[MANTIS_SHOT_MARKER, 0]));
    }

    #[test]
    fn marker_board_notifications_are_not_impacts() {
        let marker = forwarder(AccessoryType::MarkerControlBoard);
        assert!(!marker.is_shot(&[]));
        assert!(!marker.is_shot(&[MANTIS_SHOT_MARKER]));
    }

    #[test]
    fn marker_demand_requires_screen_assignment_and_keeps_screen_groups_separate() {
        use odyssey_hub_common::accessory::AccessoryInfo;

        let demand = MarkerDemand::default();
        let counts = demand.subscribe();
        let last_screen = odyssey_common::MAX_SCREEN_ID;
        let scoped = demand.acquire(Some(last_screen));
        let mut board = AccessoryInfo::with_defaults("markers", AccessoryType::MarkerControlBoard);
        let mut info = AccessoryInfoMap::from([([1; 6], board.clone())]);
        assert!(!marker_active([1; 6], &info, &counts.borrow()));
        board.screen_id = Some(last_screen);
        info.insert([1; 6], board.clone());
        info.insert([2; 6], board);
        assert!(marker_active([1; 6], &info, &counts.borrow()));
        assert!(marker_active([2; 6], &info, &counts.borrow()));
        info.get_mut(&[2; 6]).unwrap().screen_id = Some(0);
        assert!(!marker_active([2; 6], &info, &counts.borrow()));
        let all = demand.acquire(None);
        assert!(marker_active([2; 6], &info, &counts.borrow()));
        drop(scoped);
        assert!(marker_active([1; 6], &info, &counts.borrow()));
        drop(all);
        assert!(!marker_active([1; 6], &info, &counts.borrow()));
        assert!(!marker_active([2; 6], &info, &counts.borrow()));
    }

    #[test]
    fn a_shot_becomes_an_impact_on_the_assigned_device() {
        let device = Device {
            uuid: [1, 2, 3, 4, 5, 6],
            transport: Transport::Usb,
            capabilities: DeviceCapabilities::empty(),
            firmware_identity: FirmwareVersion::default(),
            firmware_version: [0; 3],
            events_transport: EventsTransport::Wired,
            events_connected: false,
            product_id: 0,
            name: Device::name_bytes(""),
            netcore_release: 0,
        };
        let device_list = DeviceList::default();
        device_list.upsert(device, None);

        let mut info = AccessoryInfo::with_defaults("mag", AccessoryType::DryFireMag);
        info.assignment = std::num::NonZeroU64::new(u64::from_le_bytes([1, 2, 3, 4, 5, 6, 0, 0]));
        let (_info_sender, info_watch) = watch::channel(HashMap::from([([0; 6], info)]));
        let (event_tx, mut events) = broadcast::channel(4);
        let forwarder = ShotForwarder {
            device_list,
            event_tx,
            info_watch,
            id: [0; 6],
            ty: AccessoryType::DryFireMag,
            radio: AccessoryRadio::System,
        };

        forwarder.forward_impact();

        let Ok(Event::DeviceEvent(DeviceEvent(device, DeviceEventKind::ImpactEvent(impact)))) =
            events.try_recv()
        else {
            panic!("expected an impact event");
        };
        assert_eq!(device.uuid, [1, 2, 3, 4, 5, 6]);
        assert_eq!(impact.timestamp, ACCESSORY_IMPACT_TIMESTAMP);
    }

    #[test]
    fn an_unassigned_accessory_forwards_nothing() {
        let forwarder = forwarder(AccessoryType::DryFireMag);
        let mut events = forwarder.event_tx.subscribe();
        forwarder.forward_impact();
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }
}
