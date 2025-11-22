use defmt::{error, info};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::signal::Signal;
use embassy_time::Duration;
use protodongers::mux::{DevicePacket, MAX_DEVICES};
use static_cell::StaticCell;

use trouble_host::Stack;

use crate::DEVICE_LIST_SUBSCRIBED;
use crate::storage::Settings;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::{AtomicBool, Ordering};

// Lightweight diagnostics
pub static DROPPED_DATA_TO_HOST: AtomicU32 = AtomicU32::new(0);
pub static DISCONNECT_COUNT: AtomicU32 = AtomicU32::new(0);

// L2CAP PSM (Protocol Service Multiplexer) for our custom protocol
pub const L2CAP_PSM_CONTROL: u16 = 0x0080;
pub const L2CAP_PSM_DATA: u16 = 0x0081;

// Pairing filter: Company Identifier and ATS product prefixes
pub const TARGET_COMPANY_ID: u16 = 1674; // 0x068A Raytac
pub const PRODUCT_ID_PREFIXES: &[&[u8]] = &[b"ATS", b"ATS-LEG"];

pub type Uuid = [u8; 6];

/// Helper to determine if a packet is a control packet (vs streaming data)
pub fn is_control_packet(pkt: &protodongers::Packet) -> bool {
    use protodongers::PacketData as P;

    match &pkt.data {
        // Control packets
        P::StreamUpdate(_)
        | P::WriteMode(_)
        | P::ReadVersion()
        | P::ReadVersionResponse(_)
        | P::ReadConfig(_)
        | P::ReadConfigResponse(_)
        | P::WriteConfig(_)
        | P::ReadProp(_)
        | P::ReadPropResponse(_)
        | P::WriteRegister(_)
        | P::ReadRegister(_)
        | P::ReadRegisterResponse(_)
        | P::ObjectReportRequest()
        | P::Ack()
        | P::FlashSettings()
        | P::Vendor(_, _) => true,

        // Streaming data packets
        P::ObjectReport(_)
        | P::CombinedMarkersReport(_)
        | P::PocMarkersReport(_)
        | P::AccelReport(_)
        | P::ImpactReport(_) => false,
    }
}

// Pairing discovery signal
pub static PAIRING_FOUND_ADDR: Signal<ThreadModeRawMutex, [u8; 6]> = Signal::new();
static PAIRING_CAPTURED: AtomicBool = AtomicBool::new(false);

// Signal to restart central loop (e.g., when bonds are cleared)
pub static RESTART_CENTRAL: Signal<ThreadModeRawMutex, ()> = Signal::new();

pub fn reset_pairing_capture() {
    PAIRING_CAPTURED.store(false, Ordering::Release);
}

pub fn report_scan_address(addr: [u8; 6]) {
    if !PAIRING_CAPTURED.swap(true, Ordering::AcqRel) {
        info!("First device found during pairing: {:02x}, signaling", addr);
        PAIRING_FOUND_ADDR.signal(addr);
    }
}

// Check adv payload for Manufacturer Specific Data matching our target
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

pub fn adv_matches_pairing(data: &[u8]) -> bool {
    adv_matches(data)
}

// Device queue and connection tracking
pub type DeviceQueue = Channel<ThreadModeRawMutex, DevicePacket, 64>;

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
            let response = protodongers::mux::MuxMsg::DevicesSnapshot(devices);
            crate::HOST_RESPONSES.send(response).await;
        }

        result
    }

    pub async fn remove(&self, uuid: &Uuid) {
        let mut map = self.connections.lock().await;
        let _ = map.remove(uuid);
        drop(map);

        if DEVICE_LIST_SUBSCRIBED.load(Ordering::Relaxed) {
            let devices = self.get_all().await;
            let response = protodongers::mux::MuxMsg::DevicesSnapshot(devices);
            crate::HOST_RESPONSES.send(response).await;
        }
    }

    pub async fn contains(&self, uuid: &Uuid) -> bool {
        let map = self.connections.lock().await;
        map.contains_key(uuid)
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

// Global references for control task
static GLOBAL_ACTIVE_CONNECTIONS: AsyncMutex<
    ThreadModeRawMutex,
    Option<&'static ActiveConnections>,
> = AsyncMutex::new(None);
static GLOBAL_DEVICE_QUEUES: AsyncMutex<ThreadModeRawMutex, Option<&'static DeviceQueues>> =
    AsyncMutex::new(None);

pub async fn set_global_refs(
    active_connections: &'static ActiveConnections,
    device_queues: &'static DeviceQueues,
) {
    *GLOBAL_ACTIVE_CONNECTIONS.lock().await = Some(active_connections);
    *GLOBAL_DEVICE_QUEUES.lock().await = Some(device_queues);
}

pub async fn get_active_connections() -> Option<&'static ActiveConnections> {
    *GLOBAL_ACTIVE_CONNECTIONS.lock().await
}

pub async fn get_device_queues() -> Option<&'static DeviceQueues> {
    *GLOBAL_DEVICE_QUEUES.lock().await
}

/// Simplified BLE Manager - just spawns device tasks
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

    /// Run the BLE central manager - spawns device tasks
    pub async fn run(&mut self) -> ! {
        info!("Starting BLE central manager");

        // Track which devices have active tasks
        let mut spawned_tasks: heapless::Vec<[u8; 6], MAX_DEVICES> = heapless::Vec::new();

        loop {
            let targets = self.settings.get_scan_targets().await;
            let pairing = crate::pairing::is_active();

            // Spawn tasks for any existing targets (not newly paired during this session)
            for addr in targets.iter() {
                if !spawned_tasks.contains(addr) {
                    info!("Spawning device task for {:02x}", *addr);
                    if self
                        .spawner
                        .spawn(crate::ble::device_task::device_connection_task(
                            self.stack,
                            self.device_packets,
                            self.active_connections,
                            self.device_queues,
                            self.settings,
                            trouble_host::prelude::BdAddr::new(*addr),
                        ))
                        .is_ok()
                    {
                        let _ = spawned_tasks.push(*addr);
                        info!("Device task spawned for {:02x}", *addr);
                    } else {
                        error!("Failed to spawn device task for {:02x} (pool full)", *addr);
                    }
                }
            }

            // Wait before next check
            let _ =
                embassy_time::with_timeout(Duration::from_secs(2), RESTART_CENTRAL.wait()).await;
        }
    }
}
