//! Accessory links own separate slots. Tracker queues and reconnect policy stay independent.
use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, with_timeout};
use odyssey_protocol::accessory::{
    self as wire, ACCESSORY_CAPACITY, Command, Completion, Error, Event, Profile, Reply, Request,
};
use odyssey_protocol::marker_control;
use odyssey_protocol::mux::MuxMsg;
use trouble_host::prelude::*;

static SCANNING: AtomicBool = AtomicBool::new(false);
pub(crate) static EPOCH: AtomicU32 = AtomicU32::new(0);
pub(crate) static TX_RESET: Signal<ThreadModeRawMutex, ()> = Signal::new();
#[derive(Clone, Copy)]
struct Queued {
    request: Request,
    received: Instant,
    epoch: u32,
}
static REQUESTS: Channel<ThreadModeRawMutex, Queued, { wire::MAX_PENDING_REQUESTS }> =
    Channel::new();
const DISCOVERY_CAPACITY: usize = 24;

#[derive(Clone, Copy)]
struct Peer {
    address: Address,
    profile: Profile,
}
static PEERS: Mutex<ThreadModeRawMutex, RefCell<heapless::Vec<Peer, DISCOVERY_CAPACITY>>> =
    Mutex::new(RefCell::new(heapless::Vec::new()));

#[derive(Clone, Copy)]
struct Session {
    address: [u8; 6],
    profile: Profile,
    connected: bool,
    closing: bool,
    disconnect: Option<(u32, u32)>,
}
static SESSIONS: Mutex<ThreadModeRawMutex, RefCell<[Option<Session>; ACCESSORY_CAPACITY]>> =
    Mutex::new(RefCell::new([None; ACCESSORY_CAPACITY]));
static STOP: [Signal<ThreadModeRawMutex, ()>; ACCESSORY_CAPACITY] =
    [const { Signal::new() }; ACCESSORY_CAPACITY];
static COMMANDS: [Channel<ThreadModeRawMutex, Queued, 2>; ACCESSORY_CAPACITY] =
    [const { Channel::new() }; ACCESSORY_CAPACITY];

fn host_address(address: BdAddr) -> [u8; 6] {
    let mut bytes = address.into_inner();
    bytes.reverse();
    bytes
}

fn reserve(
    sessions: &mut [Option<Session>; ACCESSORY_CAPACITY],
    address: [u8; 6],
    profile: Profile,
) -> Result<usize, Error> {
    if sessions.iter().flatten().any(|s| s.address == address) {
        return Err(Error::Busy);
    }
    let slot = sessions
        .iter()
        .position(Option::is_none)
        .ok_or(Error::Capacity)?;
    sessions[slot] = Some(Session {
        address,
        profile,
        connected: false,
        closing: false,
        disconnect: None,
    });
    Ok(slot)
}

fn advertised_profile(data: &[u8]) -> Option<Profile> {
    let mut rest = data;
    while let Some((&len, bytes)) = rest.split_first() {
        let len = len as usize;
        if len == 0 || len > bytes.len() {
            break;
        }
        let (field, next) = bytes.split_at(len);
        rest = next;
        if matches!(field[0], 0x06 | 0x07 | 0x21) {
            let values = if field[0] == 0x21 {
                &field[1..field.len().min(17)]
            } else {
                &field[1..]
            };
            for value in values.chunks_exact(16) {
                for profile in [
                    Profile::MarkerControlBoard,
                    Profile::DryFireMag,
                    Profile::MantisX,
                ] {
                    let mut uuid = profile.service_uuid();
                    uuid.reverse();
                    if value == uuid {
                        return Some(profile);
                    }
                }
            }
        }
    }
    let name = super::central::extract_local_name(data);
    let name = name.as_bytes();
    if name
        .get(..6)
        .is_some_and(|n| n.eq_ignore_ascii_case(b"mantis"))
    {
        Some(Profile::MantisX)
    } else if name
        .windows(10)
        .any(|n| n.eq_ignore_ascii_case(b"dryfiremag"))
    {
        Some(Profile::DryFireMag)
    } else if name.starts_with(b"Odyssey Marker") {
        Some(Profile::MarkerControlBoard)
    } else {
        None
    }
}

fn advertised_pairing_window(data: &[u8]) -> Option<u8> {
    let mut rest = data;
    let mut uuid = marker_control::SERVICE_UUID;
    uuid.reverse();
    while let Some((&len, bytes)) = rest.split_first() {
        let len = usize::from(len);
        if len == 0 || len > bytes.len() {
            return None;
        }
        let (field, next) = bytes.split_at(len);
        rest = next;
        if field[0] == 0x21 && field.get(1..17) == Some(uuid.as_slice()) {
            return marker_control::parse_pairing_state(&field[17..]);
        }
    }
    None
}

pub fn advertisement(address: Address, data: &[u8]) {
    let Some(profile) = advertised_profile(data) else {
        return;
    };
    PEERS.lock(|peers| {
        let mut peers = peers.borrow_mut();
        if let Some(peer) = peers.iter_mut().find(|p| p.address.addr == address.addr) {
            *peer = Peer { address, profile };
        } else {
            if peers.is_full() {
                peers.remove(0);
            }
            let _ = peers.push(Peer { address, profile });
        }
    });
    if SCANNING.load(Ordering::Acquire) {
        let name = super::central::extract_local_name(data);
        let mut bytes = heapless::Vec::new();
        let _ = bytes.extend_from_slice(name.as_bytes());
        event(Event::Discovered {
            address: host_address(address.addr),
            profile,
            name: bytes.into(),
        });
        if profile == Profile::MarkerControlBoard {
            if let Some(seconds_remaining) = advertised_pairing_window(data) {
                event(Event::PairingWindow {
                    address: host_address(address.addr),
                    seconds_remaining,
                });
            }
        }
    }
}

fn event(value: Event) {
    if crate::HOST_RESPONSES
        .try_send(MuxMsg::AccessoryEvent(value))
        .is_err()
    {
        // Discovery may be dropped. Data loss is surfaced for notification sessions below.
    }
}

async fn complete(id: u32, result: Result<Reply, Error>, epoch: u32) {
    send(
        MuxMsg::AccessoryCompletion(Completion { id, result }),
        epoch,
    )
    .await;
}

async fn send(mut message: MuxMsg, epoch: u32) {
    while EPOCH.load(Ordering::Acquire) == epoch {
        match crate::HOST_RESPONSES.try_send(message) {
            Ok(()) => return,
            Err(embassy_sync::channel::TrySendError::Full(returned)) => message = returned,
        }
        embassy_time::Timer::after_millis(5).await;
    }
}

pub async fn submit(request: Request) {
    let epoch = EPOCH.load(Ordering::Acquire);
    if REQUESTS
        .try_send(Queued {
            request,
            received: Instant::now(),
            epoch,
        })
        .is_err()
    {
        complete(request.id, Err(Error::Busy), epoch).await;
    }
}

pub fn usb_lost() {
    EPOCH.fetch_add(1, Ordering::AcqRel);
    TX_RESET.signal(());
    SCANNING.store(false, Ordering::Release);
    crate::HOST_RESPONSES.clear();
    REQUESTS.clear();
    SESSIONS.lock(|sessions| {
        for session in sessions.borrow_mut().iter_mut().flatten() {
            session.closing = true;
            session.disconnect = None;
        }
    });
    for (i, stop) in STOP.iter().enumerate() {
        COMMANDS[i].clear();
        stop.signal(());
    }
}

#[embassy_executor::task]
pub async fn dispatcher(
    stack: &'static trouble_host::Stack<'static, crate::Controller, crate::Pool>,
    settings: &'static crate::storage::Settings,
    trackers: &'static super::central::ActiveConnections,
    spawner: embassy_executor::Spawner,
) {
    loop {
        let queued = REQUESTS.receive().await;
        let Queued { request, epoch, .. } = queued;
        if EPOCH.load(Ordering::Acquire) != epoch {
            continue;
        }
        if queued.received.elapsed() > Duration::from_millis(wire::REQUEST_TIMEOUT_MILLIS) {
            complete(request.id, Err(Error::Timeout), epoch).await;
            continue;
        }
        let result = match request.command {
            Command::Capabilities() => {
                let (connected, connecting) = SESSIONS.lock(|s| {
                    let s = s.borrow();
                    (
                        s.iter().flatten().filter(|s| s.connected).count(),
                        s.iter()
                            .flatten()
                            .filter(|s| !s.connected && !s.closing)
                            .count(),
                    )
                });
                Ok(Reply::Capabilities(wire::Capabilities {
                    total_capacity: wire::TOTAL_CAPACITY as u8,
                    tracker_capacity: odyssey_protocol::mux::MAX_DEVICES as u8,
                    accessory_capacity: ACCESSORY_CAPACITY as u8,
                    connected_trackers: trackers.get_all().await.len() as u8,
                    connected_accessories: connected as u8,
                    connecting_accessories: connecting as u8,
                }))
            }
            Command::Scan { enabled } => {
                // Existing background scanner remains owner of controller scan sessions.
                SCANNING.store(enabled, Ordering::Release);
                if enabled {
                    PEERS.lock(|p| p.borrow_mut().clear());
                }
                Ok(Reply::Done())
            }
            Command::Connect {
                address,
                profile,
                enroll,
            } => {
                let peer = PEERS.lock(|p| {
                    p.borrow()
                        .iter()
                        .find(|p| host_address(p.address.addr) == address)
                        .copied()
                });
                let Some(peer) = peer else {
                    complete(request.id, Err(Error::UnknownPeer), epoch).await;
                    continue;
                };
                if peer.profile != profile {
                    complete(request.id, Err(Error::InvalidProfile), epoch).await;
                    continue;
                }
                if settings.get_bond(peer.address.addr).await.is_some() {
                    complete(request.id, Err(Error::InvalidProfile), epoch).await;
                    continue;
                }
                let slot = SESSIONS.lock(|s| {
                    let mut s = s.borrow_mut();
                    let i = reserve(&mut s, address, profile)?;
                    STOP[i].reset();
                    COMMANDS[i].clear();
                    Ok(i)
                });
                match slot {
                    Ok(slot) => {
                        match accessory_session(
                            stack,
                            settings,
                            slot,
                            peer,
                            enroll,
                            request.id,
                            epoch,
                            queued.received,
                        ) {
                            Ok(token) => {
                                spawner.spawn(token);
                                continue;
                            }
                            Err(_) => {
                                SESSIONS.lock(|s| s.borrow_mut()[slot] = None);
                                Err(Error::Busy)
                            }
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            Command::Disconnect { address } => {
                let result = SESSIONS.lock(|s| {
                    let mut s = s.borrow_mut();
                    let (i, state) = s
                        .iter_mut()
                        .enumerate()
                        .find_map(|(i, s)| {
                            s.as_mut().filter(|s| s.address == address).map(|s| (i, s))
                        })
                        .ok_or(Error::NotConnected)?;
                    if state.disconnect.is_some() {
                        return Err(Error::Busy);
                    }
                    state.disconnect = Some((request.id, epoch));
                    STOP[i].signal(());
                    Ok(())
                });
                match result {
                    Ok(()) => continue,
                    Err(e) => Err(e),
                }
            }
            Command::MarkerDemand { address, .. }
            | Command::MarkerMode { address, .. }
            | Command::MarkerOta { address, .. }
            | Command::MarkerOtaStatus { address } => {
                let slot = SESSIONS.lock(|s| {
                    s.borrow()
                        .iter()
                        .enumerate()
                        .find_map(|(i, s)| s.filter(|s| s.address == address).map(|s| (i, s)))
                });
                match slot {
                    Some((_, s)) if s.profile != Profile::MarkerControlBoard => {
                        Err(Error::InvalidProfile)
                    }
                    Some((i, s)) if s.connected && !s.closing && s.disconnect.is_none() => {
                        if COMMANDS[i].try_send(queued).is_ok() {
                            continue;
                        }
                        Err(Error::Busy)
                    }
                    _ => Err(Error::NotConnected),
                }
            }
        };
        complete(request.id, result, epoch).await;
    }
}

async fn secure_marker(
    conn: &Connection<'static, crate::Pool>,
    settings: &crate::storage::Settings,
    peer: Peer,
    enroll: bool,
    slot: usize,
    token: u64,
) -> Result<(), Error> {
    let old_bond = settings
        .get_accessory_bond(peer.address.addr.into_inner())
        .await
        .filter(|b| b.is_bonded && b.security_level > 0);
    if !enroll && old_bond.is_none() {
        return Err(Error::Security);
    }
    conn.set_bondable(enroll).map_err(|_| Error::Security)?;
    conn.request_security().map_err(|_| Error::Security)?;
    loop {
        match conn.next().await {
            ConnectionEvent::PairingComplete {
                security_level,
                bond,
            } => {
                if security_level == SecurityLevel::NoEncryption {
                    return Err(Error::Security);
                }
                if let Some(bond) = bond {
                    if !bond.is_bonded {
                        return Err(Error::Security);
                    }
                    let bond = super::security::bonddata_from_info(&bond);
                    if enroll {
                        crate::storage::persist_accessory_bond(bond, slot, token)
                            .await
                            .map_err(|_| Error::BondStorage)?;
                    } else if !old_bond
                        .is_some_and(|old| old.ltk == bond.ltk && old.bd_addr == bond.bd_addr)
                    {
                        return Err(Error::Security);
                    }
                } else if enroll || old_bond.is_none() {
                    return Err(Error::Security);
                }
                return Ok(());
            }
            ConnectionEvent::PairingFailed(_) => return Err(Error::Security),
            ConnectionEvent::Disconnected { .. } => return Err(Error::Disconnected),
            _ => {}
        }
    }
}

#[embassy_executor::task(pool_size = odyssey_protocol::accessory::ACCESSORY_CAPACITY)]
async fn accessory_session(
    stack: &'static trouble_host::Stack<'static, crate::Controller, crate::Pool>,
    settings: &'static crate::storage::Settings,
    slot: usize,
    peer: Peer,
    enroll: bool,
    connect_id: u32,
    epoch: u32,
    received: Instant,
) {
    let conn = RefCell::new(None::<Connection<'static, crate::Pool>>);
    let connected = Cell::new(false);
    let pending = Cell::new(None::<u32>);
    let mut ota_characteristic: Option<Characteristic<u8>> = None;
    let mut ota_characteristic_checked = false;
    let run = async {
        let mut central = stack.build().central;
        let config = ConnectConfig {
            connect_params: ConnectParams {
                min_connection_interval: Duration::from_millis(30),
                max_connection_interval: Duration::from_millis(50),
                ..Default::default()
            },
            scan_config: ScanConfig {
                filter_accept_list: &[(peer.address.kind, &peer.address.addr)],
                ..Default::default()
            },
        };
        let lease = super::central::begin_connect().await;
        let connection = with_timeout(Duration::from_secs(10), central.connect(&config))
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|_| Error::Disconnected)?;
        drop(lease);
        let mut guard = conn.borrow_mut();
        let connection = guard.insert(connection);
        if peer.profile == Profile::MarkerControlBoard {
            with_timeout(
                Duration::from_secs(15),
                secure_marker(
                    connection,
                    settings,
                    peer,
                    enroll,
                    slot,
                    (u64::from(epoch) << 32) | u64::from(connect_id),
                ),
            )
            .await
            .map_err(|_| Error::Timeout)??;
        } else {
            connection
                .set_bondable(false)
                .map_err(|_| Error::Security)?;
        }
        let client = with_timeout(
            Duration::from_secs(5),
            GattClient::<_, _, 4>::new(stack, connection),
        )
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(|_| Error::Gatt)?;
        let operations = async {
            let setup = async {
                let services = client
                    .services_by_uuid(&Uuid::from(u128::from_be_bytes(
                        peer.profile.service_uuid(),
                    )))
                    .await
                    .map_err(|_| Error::Gatt)?;
                let service = services.first().ok_or(Error::InvalidProfile)?;
                let characteristic: Characteristic<u8> = client
                    .characteristic_by_uuid(
                        service,
                        &Uuid::from(u128::from_be_bytes(peer.profile.characteristic_uuid())),
                    )
                    .await
                    .map_err(|_| Error::Gatt)?;
                Ok::<_, Error>((service.clone(), characteristic))
            };
            let (service, characteristic) = with_timeout(Duration::from_secs(5), setup)
                .await
                .map_err(|_| Error::Timeout)??;
            let mut listener = if peer.profile == Profile::MarkerControlBoard {
                // A successful OFF write means the board accepted the persisted owner.
                // SMP encryption alone does not show that.
                with_timeout(
                    Duration::from_secs(5),
                    client.write_characteristic(&characteristic, &[0]),
                )
                .await
                .map_err(|_| Error::Timeout)?
                .map_err(|_| Error::Security)?;
                None
            } else {
                Some(
                    with_timeout(
                        Duration::from_secs(5),
                        client.subscribe(&characteristic, false),
                    )
                    .await
                    .map_err(|_| Error::Timeout)?
                    .map_err(|_| Error::Gatt)?,
                )
            };
            SESSIONS.lock(|s| {
                if let Some(s) = &mut s.borrow_mut()[slot] {
                    s.connected = true;
                }
            });
            complete(connect_id, Ok(Reply::Done()), epoch).await;
            connected.set(true);
            send(
                MuxMsg::AccessoryEvent(Event::Connected {
                    address: host_address(peer.address.addr),
                    profile: peer.profile,
                }),
                epoch,
            )
            .await;
            if let Some(listener) = listener.as_mut() {
                loop {
                    let notification = listener.next().await;
                    let mut data = heapless::Vec::new();
                    if data.extend_from_slice(notification.as_ref()).is_err() {
                        return Err(Error::NotificationTooLarge);
                    }
                    // Backpressure must not silently turn shot loss into a healthy session.
                    if crate::HOST_RESPONSES
                        .try_send(MuxMsg::AccessoryEvent(Event::Notification {
                            address: host_address(peer.address.addr),
                            profile: peer.profile,
                            data: data.into(),
                        }))
                        .is_err()
                    {
                        return Err(Error::Busy);
                    }
                }
            }
            loop {
                let queued = COMMANDS[slot].receive().await;
                let request = queued.request;
                pending.set(Some(request.id));
                let result = match request.command {
                    Command::MarkerDemand { active, .. } => {
                        if queued.received.elapsed()
                            > Duration::from_millis(marker_control::RENEW_MILLIS)
                        {
                            Err(Error::Timeout)
                        } else {
                            with_timeout(
                                Duration::from_secs(3),
                                client.write_characteristic(&characteristic, &[u8::from(active)]),
                            )
                            .await
                            .map_err(|_| Error::Timeout)
                            .and_then(|result| result.map(|()| Reply::Done()).map_err(|_| Error::Gatt))
                        }
                    }
                    Command::MarkerMode { mode, .. } => {
                        let write = async {
                            let mode_characteristic: Characteristic<u8> = client
                                .characteristic_by_uuid(
                                    &service,
                                    &Uuid::from(u128::from_be_bytes(marker_control::MODE_UUID)),
                                )
                                .await
                                .map_err(|_| Error::Gatt)?;
                            client
                                .write_characteristic(&mode_characteristic, &[mode as u8])
                                .await
                                .map(|()| Reply::Done())
                                .map_err(|_| Error::Gatt)
                        };
                        with_timeout(Duration::from_secs(5), write)
                            .await
                            .map_err(|_| Error::Timeout)
                            .and_then(|result| result)
                    }
                    Command::MarkerOta { frame, .. } => {
                        let frame_len = usize::from(frame.len);
                        if frame_len == 0
                            || frame_len > odyssey_protocol::ota::MAX_FRAME
                            || frame_len > frame.data.len()
                        {
                            complete(request.id, Err(Error::Gatt), epoch).await;
                            pending.set(None);
                            continue;
                        }
                        if !ota_characteristic_checked {
                            ota_characteristic_checked = true;
                            ota_characteristic = match client
                                .characteristic_by_uuid(
                                    &service,
                                    &Uuid::from(u128::from_be_bytes(marker_control::OTA_UUID)),
                                )
                                .await
                            {
                                Ok(characteristic) => Some(characteristic),
                                Err(BleHostError::BleHost(trouble_host::Error::NotFound)) => None,
                                Err(_) => return Err(Error::Gatt),
                            };
                        }
                        let Some(characteristic) = ota_characteristic.as_ref() else {
                            complete(request.id, Err(Error::Unsupported), epoch).await;
                            pending.set(None);
                            continue;
                        };
                        match with_timeout(
                            Duration::from_secs(5),
                            client.write_characteristic(characteristic, &frame.data[..frame_len]),
                        )
                        .await
                        {
                            Err(_) => Err(Error::Timeout),
                            Ok(Err(BleHostError::BleHost(trouble_host::Error::Att(_)))) => {
                                complete(request.id, Err(Error::Gatt), epoch).await;
                                pending.set(None);
                                continue;
                            }
                            Ok(Err(_)) => Err(Error::Gatt),
                            Ok(Ok(())) => Ok(Reply::Done()),
                        }
                    }
                    Command::MarkerOtaStatus { .. } => {
                        let read = async {
                            if !ota_characteristic_checked {
                                ota_characteristic_checked = true;
                                ota_characteristic = match client
                                    .characteristic_by_uuid(
                                        &service,
                                        &Uuid::from(u128::from_be_bytes(marker_control::OTA_UUID)),
                                    )
                                    .await
                                {
                                    Ok(characteristic) => Some(characteristic),
                                    Err(BleHostError::BleHost(trouble_host::Error::NotFound)) => None,
                                    Err(_) => return Err(Error::Gatt),
                                };
                            }
                            let Some(characteristic) = ota_characteristic.as_ref() else {
                                return Err(Error::Unsupported);
                            };
                            let mut status = [0; odyssey_protocol::ota::STATUS_LEN];
                            let len = client
                                .read_characteristic(characteristic, &mut status)
                                .await
                                .map_err(|_| Error::Gatt)?;
                            if len != status.len() {
                                return Err(Error::Gatt);
                            }
                            Ok(Reply::MarkerOtaStatus(status))
                        };
                        with_timeout(Duration::from_secs(5), read)
                            .await
                            .map_err(|_| Error::Timeout)
                            .and_then(|result| result)
                    }
                    _ => Err(Error::Unsupported),
                };
                if matches!(result, Err(Error::Unsupported)) {
                    complete(request.id, result, epoch).await;
                    pending.set(None);
                    continue;
                }
                complete(request.id, result, epoch).await;
                pending.set(None);
                if let Err(e) = result {
                    return Err(e);
                }
            }
        };
        let connection_events = async {
            loop {
                match connection.next().await {
                    ConnectionEvent::Disconnected { .. } => return Error::Disconnected,
                    ConnectionEvent::PairingFailed(_) => return Error::Security,
                    _ => {}
                }
            }
        };
        match select(connection_events, select(client.task(), operations)).await {
            Either::First(error) => Err(error),
            Either::Second(Either::First(_)) => Err(Error::Gatt),
            Either::Second(Either::Second(result)) => result,
        }
    };
    let setup_deadline = async {
        embassy_time::Timer::at(
            received + Duration::from_millis(wire::REQUEST_TIMEOUT_MILLIS - 3_000),
        )
        .await;
        if connected.get() {
            core::future::pending::<()>().await;
        }
    };
    let result = match select(STOP[slot].wait(), select(setup_deadline, run)).await {
        Either::First(()) => Err(Error::Disconnected),
        Either::Second(Either::First(())) => Err(Error::Timeout),
        Either::Second(Either::Second(result)) => result,
    };
    SESSIONS.lock(|s| {
        if let Some(s) = &mut s.borrow_mut()[slot] {
            s.closing = true;
        }
    });
    let disconnect_result = if let Some(connection) = conn.borrow_mut().take() {
        connection.disconnect();
        with_timeout(Duration::from_secs(3), async {
            while connection.is_connected() {
                embassy_time::Timer::after_millis(5).await;
            }
        })
        .await
        .map(|()| Reply::Done())
        .map_err(|_| Error::Timeout)
    } else {
        Ok(Reply::Done())
    };
    if peer.profile == Profile::MarkerControlBoard {
        let saved = settings
            .get_accessory_bond(peer.address.addr.into_inner())
            .await;
        for bond in stack.get_bond_information() {
            if bond.identity.bd_addr == peer.address.addr {
                let _ = stack.remove_bond_information(bond.identity);
            }
        }
        if let Some(bond) = saved {
            let _ = stack.add_bond_information(super::security::info_from_bonddata(&bond));
        }
    }
    SESSIONS.lock(|s| {
        if let Some(s) = &mut s.borrow_mut()[slot] {
            s.connected = false;
        }
    });
    if !connected.get() {
        complete(connect_id, result, epoch).await;
    }
    if let Some(id) = pending.get() {
        complete(id, Err(Error::Disconnected), epoch).await;
    }
    while let Ok(queued) = COMMANDS[slot].try_receive() {
        complete(queued.request.id, Err(Error::Disconnected), epoch).await;
    }
    if EPOCH.load(Ordering::Acquire) == epoch {
        if let Err(error) = result {
            send(
                MuxMsg::AccessoryEvent(Event::Error {
                    address: host_address(peer.address.addr),
                    error,
                }),
                epoch,
            )
            .await;
        }
        send(
            MuxMsg::AccessoryEvent(Event::Disconnected {
                address: host_address(peer.address.addr),
            }),
            epoch,
        )
        .await;
    }
    let disconnect_id = SESSIONS.lock(|s| s.borrow()[slot].and_then(|s| s.disconnect));
    if let Some((id, request_epoch)) = disconnect_id {
        complete(id, disconnect_result, request_epoch).await;
    }
    SESSIONS.lock(|s| s.borrow_mut()[slot] = None);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accessory_budget_never_takes_tracker_slots() {
        let mut sessions = [None; ACCESSORY_CAPACITY];
        for i in 0..ACCESSORY_CAPACITY {
            assert_eq!(
                reserve(&mut sessions, [i as u8; 6], Profile::DryFireMag),
                Ok(i)
            );
        }
        assert_eq!(
            reserve(&mut sessions, [0; 6], Profile::DryFireMag),
            Err(Error::Busy)
        );
        assert_eq!(
            reserve(&mut sessions, [99; 6], Profile::DryFireMag),
            Err(Error::Capacity)
        );
        sessions[1] = None;
        assert_eq!(reserve(&mut sessions, [99; 6], Profile::MantisX), Ok(1));
        assert_eq!(
            wire::TOTAL_CAPACITY - sessions.iter().flatten().count(),
            odyssey_protocol::mux::MAX_DEVICES
        );
    }

    #[test]
    fn name_only_discovery_and_host_address_order() {
        assert_eq!(
            advertised_profile(b"\x08\x09mAnTiSx"),
            Some(Profile::MantisX)
        );
        assert_eq!(
            advertised_profile(b"\x0b\x09DryFireMag"),
            Some(Profile::DryFireMag)
        );
        assert_eq!(advertised_profile(b"\x08\x09not-a-peer"), None);
        assert_eq!(
            host_address(BdAddr::new([1, 2, 3, 4, 5, 6])),
            [6, 5, 4, 3, 2, 1]
        );
    }

    #[test]
    fn advertisements_require_whole_matching_uuid() {
        for profile in [
            Profile::MarkerControlBoard,
            Profile::DryFireMag,
            Profile::MantisX,
        ] {
            let mut data = [0; 18];
            data[0] = 17;
            data[1] = 7;
            let mut uuid = profile.service_uuid();
            uuid.reverse();
            data[2..].copy_from_slice(&uuid);
            assert_eq!(advertised_profile(&data), Some(profile));
            assert_eq!(advertised_profile(&data[..17]), None);
            data[0] = 255;
            assert_eq!(advertised_profile(&data), None);
        }
    }

    #[test]
    fn pairing_window_requires_marker_service_data_and_valid_payload() {
        let mut data = [0; 21];
        data[0] = 19;
        data[1] = 0x21;
        let mut uuid = marker_control::SERVICE_UUID;
        uuid.reverse();
        data[2..18].copy_from_slice(&uuid);
        data[18] = marker_control::PAIRING_STATE_VERSION;
        data[19] = marker_control::ENROLLMENT_SECONDS;
        assert_eq!(advertised_pairing_window(&data), Some(30));
        assert_eq!(advertised_profile(&data), Some(Profile::MarkerControlBoard));
        data[19] = 0;
        assert_eq!(advertised_pairing_window(&data), Some(0));
        data[19] = 31;
        assert_eq!(advertised_pairing_window(&data), None);
        data[19] = 1;
        data[18] += 1;
        assert_eq!(advertised_pairing_window(&data), None);
        data[18] = marker_control::PAIRING_STATE_VERSION;
        for end in 0..20 {
            assert_eq!(advertised_pairing_window(&data[..end]), None);
        }
        data[0] = 20;
        assert_eq!(advertised_pairing_window(&data), None);
        data[0] = 19;
        data[2] ^= 1;
        assert_eq!(advertised_pairing_window(&data), None);
        data[2] ^= 1;
        data[1] = 7;
        assert_eq!(advertised_pairing_window(&data), None);
        assert_eq!(advertised_pairing_window(b"\x0f\x09Odyssey Marker"), None);
    }
}
