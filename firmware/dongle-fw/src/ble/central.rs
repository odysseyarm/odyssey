use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use defmt::{error, info};
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, ThreadModeRawMutex};
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::signal::Signal;
use embassy_time::Duration;
use odyssey_protocol::mux::{DevicePacket, MAX_DEVICES};
use static_cell::StaticCell;
use trouble_host::Stack;

use crate::DEVICE_LIST_SUBSCRIBED;
use crate::ble::task_slots::{self, ClaimError};
use crate::storage::Settings;

// Counters read when diagnosing link drops and host backpressure.
pub static DROPPED_DATA_TO_HOST: AtomicU32 = AtomicU32::new(0);
pub static DISCONNECT_COUNT: AtomicU32 = AtomicU32::new(0);

// L2CAP PSMs for the ATS control and data channels. 0x0080 and up is the
// dynamic LE PSM range.
pub const L2CAP_PSM_CONTROL: u16 = 0x0080;
pub const L2CAP_PSM_DATA: u16 = 0x0081;

// Pairing only accepts advertisements whose Manufacturer Specific Data carries
// this company ID followed by one of these product prefixes.
pub const TARGET_COMPANY_ID: u16 = 1674; // 0x068A Raytac
pub const PRODUCT_ID_PREFIXES: &[&[u8]] = &[b"ATS", b"ATS-LEG"];

pub type Uuid = [u8; 6];

pub static PAIRING_FOUND_ADDR: Signal<ThreadModeRawMutex, [u8; 6]> = Signal::new();
static PAIRING_CAPTURED: AtomicBool = AtomicBool::new(false);

// Bind the captured name to its address: another device can finish bonding
// while this pairing capture remains the latest one.
static PAIRING_FOUND_NAME: BlockingMutex<
    CriticalSectionRawMutex,
    RefCell<Option<([u8; 6], heapless::String<32>)>>,
> = BlockingMutex::new(RefCell::new(None));

// BLE names by address, refreshed from every advertisement seen.
static DEVICE_NAMES: BlockingMutex<
    CriticalSectionRawMutex,
    RefCell<heapless::LinearMap<[u8; 6], heapless::String<32>, MAX_DEVICES>>,
> = BlockingMutex::new(RefCell::new(heapless::LinearMap::new()));

pub fn update_device_name(addr: [u8; 6], name: heapless::String<32>) {
    DEVICE_NAMES.lock(|m| {
        let mut map = m.borrow_mut();
        if let Some(existing) = map.get_mut(&addr) {
            *existing = name;
        } else {
            let _ = map.insert(addr, name);
        }
    });
}

pub fn clear_device_names() {
    DEVICE_NAMES.lock(|m| m.borrow_mut().clear());
}

// Wakes the central loop early, e.g. after bonds are cleared.
pub static RESTART_CENTRAL: Signal<ThreadModeRawMutex, ()> = Signal::new();

// CONNECT_PENDING counts device tasks inside central.connect(). The background
// scanner pauses while it is nonzero, because the controller rejects a connect
// while a scan is running.
pub static CONNECT_PENDING: AtomicU8 = AtomicU8::new(0);
pub static SCAN_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static SCAN_STOPPED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static SCAN_RESUME: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static CONNECT_LOCK: AsyncMutex<ThreadModeRawMutex, ()> = AsyncMutex::new(());

/// Registers a pending connect and waits until the background scanner has
/// stopped its scan session. Call before `central.connect()` and drop the
/// returned lease once it returns, success or failure.
pub async fn begin_connect() -> ConnectLease {
    let guard = CONNECT_LOCK.lock().await;
    CONNECT_PENDING.fetch_add(1, Ordering::Release);
    let lease = ConnectLease { _guard: guard };
    while SCAN_ACTIVE.load(Ordering::Acquire) {
        SCAN_STOPPED.wait().await;
    }
    lease
}

/// A pending connect counted in [`CONNECT_PENDING`]. Dropping it releases the
/// count, also when the device task is stopped mid-connect. The last one out
/// resumes the background scanner.
pub struct ConnectLease {
    _guard: embassy_sync::mutex::MutexGuard<'static, ThreadModeRawMutex, ()>,
}

impl Drop for ConnectLease {
    fn drop(&mut self) {
        let prev = CONNECT_PENDING.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            SCAN_RESUME.signal(());
        }
    }
}

pub fn reset_pairing_capture() {
    PAIRING_FOUND_ADDR.reset();
    PAIRING_CAPTURED.store(false, Ordering::Release);
    PAIRING_FOUND_NAME.lock(|n| *n.borrow_mut() = None);
}

pub fn report_scan_address(addr: [u8; 6], name: heapless::String<32>) {
    if !PAIRING_CAPTURED.swap(true, Ordering::AcqRel) {
        info!("First device found during pairing: {:02x}, signaling", addr);
        PAIRING_FOUND_NAME.lock(|n| *n.borrow_mut() = Some((addr, name)));
        PAIRING_FOUND_ADDR.signal(addr);
    }
}

/// Name last advertised by `addr`, or empty if it has not been seen.
pub fn get_cached_device_name(addr: &[u8; 6]) -> heapless::String<32> {
    DEVICE_NAMES.lock(|m| m.borrow().get(addr).cloned().unwrap_or_default())
}

/// Name captured for `addr` during the last pairing scan, or empty for any other address.
pub fn get_pairing_name(addr: &[u8; 6]) -> heapless::String<32> {
    PAIRING_FOUND_NAME.lock(|n| {
        n.borrow()
            .as_ref()
            .filter(|(captured, _)| captured == addr)
            .map(|(_, name)| name.clone())
            .unwrap_or_default()
    })
}

/// Parses the local name (AD type 0x08 shortened or 0x09 complete) from AD
/// structures. Non-printable bytes are dropped.
pub fn extract_local_name(data: &[u8]) -> heapless::String<32> {
    let mut i = 0;
    while i < data.len() {
        let len = data[i] as usize;
        if len == 0 {
            break;
        }
        if i + 1 + len > data.len() {
            break;
        }
        let ty = data[i + 1];
        if ty == 0x08 || ty == 0x09 {
            let name_bytes = &data[(i + 2)..(i + 1 + len)];
            let mut s: heapless::String<32> = heapless::String::new();
            for &b in name_bytes.iter().take(32) {
                if b >= 0x20 && b < 0x7F {
                    let _ = s.push(b as char);
                }
            }
            return s;
        }
        i += 1 + len;
    }
    heapless::String::new()
}

/// True when the advertisement carries Manufacturer Specific Data with
/// [`TARGET_COMPANY_ID`] and one of [`PRODUCT_ID_PREFIXES`].
pub fn adv_matches(data: &[u8]) -> bool {
    let mut i = 0;
    while i < data.len() {
        let len = data[i] as usize;
        if len == 0 {
            break;
        }
        if i + 1 + len > data.len() {
            break;
        }
        let ty = data[i + 1];
        if ty == 0xFF {
            let msd = &data[(i + 2)..(i + 1 + len)];
            if msd.len() >= 2 {
                let cid = u16::from_le_bytes([msd[0], msd[1]]);
                if cid == TARGET_COMPANY_ID {
                    if PRODUCT_ID_PREFIXES.is_empty() {
                        return true;
                    }
                    for p in PRODUCT_ID_PREFIXES {
                        if msd.len() >= 2 + p.len() && &msd[2..(2 + p.len())] == *p {
                            return true;
                        }
                    }
                }
            }
        }
        i += 1 + len;
    }
    false
}

pub type DeviceQueue = Channel<ThreadModeRawMutex, DevicePacket, 32>;

pub struct DeviceQueues {
    queues: AsyncMutex<
        ThreadModeRawMutex,
        heapless::LinearMap<Uuid, (&'static DeviceQueue, usize), MAX_DEVICES>,
    >,
}

static QUEUE_POOL: [DeviceQueue; MAX_DEVICES] = {
    const INIT: DeviceQueue = Channel::new();
    [INIT; MAX_DEVICES]
};
static ALLOCATED: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

impl DeviceQueues {
    pub const fn new() -> Self {
        Self {
            queues: AsyncMutex::new(heapless::LinearMap::new()),
        }
    }

    /// Hands `uuid` a free queue from the pool, emptied of anything left by
    /// the device that held it before.
    pub async fn register(&self, uuid: &Uuid) -> Option<&'static DeviceQueue> {
        let mut map = self.queues.lock().await;
        if map.contains_key(uuid) {
            return None;
        }

        let mut idx = None;
        for i in 0..MAX_DEVICES {
            let mask = 1u8 << i;
            let current = ALLOCATED.fetch_or(mask, Ordering::AcqRel);
            if (current & mask) == 0 {
                idx = Some(i);
                break;
            }
        }

        let idx = idx?;
        let queue = &QUEUE_POOL[idx];
        queue.clear();
        let _ = map.insert(*uuid, (queue, idx));
        Some(queue)
    }

    pub async fn get(&self, uuid: &Uuid) -> Option<&'static DeviceQueue> {
        let map = self.queues.lock().await;
        map.get(uuid).map(|(q, _)| *q)
    }

    pub async fn get_queue(&self, uuid: &Uuid) -> Option<&'static DeviceQueue> {
        self.get(uuid).await
    }

    pub async fn unregister(&self, uuid: &Uuid) {
        let mut map = self.queues.lock().await;
        if let Some((_, idx)) = map.remove(uuid) {
            let mask = 1u8 << idx;
            ALLOCATED.fetch_and(!mask, Ordering::AcqRel);
        }
    }
}

pub static DEVICE_QUEUES: StaticCell<DeviceQueues> = StaticCell::new();

pub struct ActiveConnections {
    connections: AsyncMutex<ThreadModeRawMutex, heapless::LinearMap<Uuid, (), MAX_DEVICES>>,
}

impl ActiveConnections {
    pub const fn new() -> Self {
        Self {
            connections: AsyncMutex::new(heapless::LinearMap::new()),
        }
    }

    pub async fn add(&self, uuid: &Uuid) -> bool {
        let mut map = self.connections.lock().await;
        let result = map.insert(*uuid, ()).is_ok();
        drop(map);

        if result && DEVICE_LIST_SUBSCRIBED.load(Ordering::Relaxed) {
            let devices = self.get_all().await;
            let response = odyssey_protocol::mux::MuxMsg::DevicesSnapshot(devices.into());
            if crate::HOST_RESPONSES.try_send(response).is_err() {
                defmt::warn!("HOST_RESPONSES full, dropped DevicesSnapshot (add)");
            }
        }

        result
    }

    pub async fn remove(&self, uuid: &Uuid) {
        let mut map = self.connections.lock().await;
        let removed = map.remove(uuid).is_some();
        drop(map);

        if removed && DEVICE_LIST_SUBSCRIBED.load(Ordering::Relaxed) {
            let devices = self.get_all().await;
            let response = odyssey_protocol::mux::MuxMsg::DevicesSnapshot(devices.into());
            if crate::HOST_RESPONSES.try_send(response).is_err() {
                defmt::warn!("HOST_RESPONSES full, dropped DevicesSnapshot (remove)");
            }
        }
    }

    #[allow(dead_code)]
    pub async fn count(&self) -> usize {
        let map = self.connections.lock().await;
        map.len()
    }

    pub async fn get_all(&self) -> heapless::Vec<Uuid, MAX_DEVICES> {
        let map = self.connections.lock().await;
        let mut result = heapless::Vec::new();
        for (uuid, _) in map.iter() {
            let _ = result.push(*uuid);
        }
        result
    }
}

pub static ACTIVE_CONNECTIONS: StaticCell<ActiveConnections> = StaticCell::new();

/// Spawns one connection task per scan target.
pub struct BleManager {
    device_packets: &'static crate::ble::DevicePacketChannel,
    device_queues: &'static DeviceQueues,
    settings: &'static Settings,
    active_connections: &'static ActiveConnections,
    stack: &'static Stack<'static, crate::Controller, crate::Pool>,
    spawner: embassy_executor::Spawner,
}

impl BleManager {
    pub fn new(
        device_packets: &'static crate::ble::DevicePacketChannel,
        device_queues: &'static DeviceQueues,
        settings: &'static Settings,
        active_connections: &'static ActiveConnections,
        stack: &'static Stack<'static, crate::Controller, crate::Pool>,
        spawner: embassy_executor::Spawner,
    ) -> Self {
        info!("BleManager::new called");
        Self {
            device_packets,
            device_queues,
            settings,
            active_connections,
            stack,
            spawner,
        }
    }

    /// Re-reads the scan targets every 2 s, or on [`RESTART_CENTRAL`], stops
    /// the task of every address that left them, and spawns a task for each
    /// target whose previous task, if any, has exited.
    pub async fn run(&mut self) -> ! {
        info!("Starting BLE central manager");

        loop {
            let targets = self.settings.get_scan_targets().await;
            task_slots::stop_untargeted(&targets);

            for addr in targets.iter() {
                let slot = match task_slots::claim(*addr) {
                    Ok(slot) => slot,
                    Err(ClaimError::Running) => continue,
                    Err(ClaimError::Full) => {
                        error!("No free device task slot for {:02x}", *addr);
                        continue;
                    }
                };
                info!("Spawning device task for {:02x}", *addr);
                match crate::ble::device_task::device_connection_task(
                    self.stack,
                    self.device_packets,
                    self.active_connections,
                    self.device_queues,
                    self.settings,
                    slot,
                    trouble_host::prelude::BdAddr::new(*addr),
                ) {
                    Ok(token) => {
                        self.spawner.spawn(token);
                        info!("Device task spawned for {:02x}", *addr);
                    }
                    Err(_) => {
                        task_slots::release(slot);
                        error!("Failed to spawn device task for {:02x} (pool full)", *addr);
                    }
                }
            }

            let _ =
                embassy_time::with_timeout(Duration::from_secs(2), RESTART_CENTRAL.wait()).await;
        }
    }
}
