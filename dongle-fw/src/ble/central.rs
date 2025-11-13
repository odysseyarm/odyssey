use defmt::{error, info, warn};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, Instant};
use protodongers::mux::{DevicePacket, MAX_DEVICES};
use static_cell::StaticCell;

use trouble_host::prelude::{
    AddrKind, BdAddr, Central, ConnectConfig, ConnectParams, CreditFlowPolicy, L2capChannel,
    L2capChannelConfig, PhySet, ScanConfig,
};
use trouble_host::scan::Scanner;
use trouble_host::{PacketPool, Stack};

use crate::storage::Settings;
use crate::{DEVICE_LIST_SUBSCRIBED, HOST_RESPONSES};
use core::sync::atomic::{AtomicBool, Ordering};
use core::sync::atomic::AtomicU32;

// Lightweight diagnostics
static DROPPED_DATA_TO_HOST: AtomicU32 = AtomicU32::new(0);
static DISCONNECT_COUNT: AtomicU32 = AtomicU32::new(0);

// L2CAP PSM (Protocol Service Multiplexer) for our custom protocol
// Using two separate channels for control and data
const L2CAP_PSM_CONTROL: u16 = 0x0080; // Dynamic PSM for control packets
const L2CAP_PSM_DATA: u16 = 0x0081; // Dynamic PSM for data/streaming packets

// Pairing filter: Company Identifier and ATS product prefixes
pub const TARGET_COMPANY_ID: u16 = 1674; // 0x068A Raytac (example)
pub const PRODUCT_ID_PREFIXES: &[&[u8]] = &[b"ATS", b"ATS-LEG"];

pub type Uuid = [u8; 6];

/// Helper to determine if a packet is a control packet (vs streaming data)
fn is_control_packet(pkt: &protodongers::Packet) -> bool {
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

// Pairing discovery signal and helpers (first seen device during pairing)
pub static PAIRING_FOUND_ADDR: Signal<ThreadModeRawMutex, [u8; 6]> = Signal::new();
static PAIRING_CAPTURED: AtomicBool = AtomicBool::new(false);

// Signal to restart central loop (e.g., when bonds are cleared)
pub static RESTART_CENTRAL: Signal<ThreadModeRawMutex, ()> = Signal::new();

pub fn reset_pairing_capture() {
    PAIRING_CAPTURED.store(false, Ordering::Release);
}

pub fn report_scan_address(addr: [u8; 6]) {
    if !PAIRING_CAPTURED.swap(true, Ordering::AcqRel) {
        PAIRING_FOUND_ADDR.signal(addr);
    }
}

// Check adv payload for Manufacturer Specific Data (type 0xFF) matching our target CID and product prefix
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
            // Manufacturer Specific Data
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

/// More permissive match used during pairing: accept any advertisement that
/// includes Manufacturer Specific Data (MSD). This allows discovering the VM
/// when it exposes MSD only in the scan response during pairing.
pub fn adv_matches_pairing(data: &[u8]) -> bool {
    // During pairing, require our company ID and the ATS/ATS-LEG prefixes, same as normal filter.
    adv_matches(data)
}

// Simplified device queue and connection tracking for trouble-host
pub type DeviceQueue = Channel<ThreadModeRawMutex, DevicePacket, 64>;

pub struct DeviceQueues {
    queues: AsyncMutex<
        ThreadModeRawMutex,
        heapless::LinearMap<Uuid, (&'static DeviceQueue, usize), MAX_DEVICES>,
    >,
}

// Pre-allocated array of queues that can be reused
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

        // Find first free slot using atomic bitmap
        let mut idx = None;
        for i in 0..MAX_DEVICES {
            let mask = 1u8 << i;
            let current = ALLOCATED.fetch_or(mask, Ordering::AcqRel);
            if (current & mask) == 0 {
                // This slot was free, we claimed it
                idx = Some(i);
                break;
            }
        }
        
        let idx = idx?;
        
        // Get reference to the pre-allocated queue
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
            // Free the slot in the bitmap
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
        info!("ActiveConnections::add() called for device {:02x}", uuid);

        let mut map = self.connections.lock().await;
        let result = map.insert(*uuid, ()).is_ok();
        drop(map); // Release lock before sending notification

        info!("Device add result: {}", result);

        // Send notification if subscribed
        if result && DEVICE_LIST_SUBSCRIBED.load(Ordering::Relaxed) {
            info!("Device list subscription flag: true");
            let devices = self.get_all().await;
            let device_count = devices.len();
            info!("Building snapshot with {} devices", device_count);
            let response = protodongers::mux::MuxMsg::DevicesSnapshot(devices);
            HOST_RESPONSES.send(response).await;
            info!("Sent DevicesSnapshot notification with {} devices", device_count);
        } else if !result {
            info!("Skipping snapshot notification - device was already present");
        } else {
            info!("Skipping snapshot notification - no subscription");
        }

        result
    }

    pub async fn remove(&self, uuid: &Uuid) {
        info!("ActiveConnections::remove() called for device {:02x}", uuid);
        
        let mut map = self.connections.lock().await;
        let _ = map.remove(uuid);
        drop(map); // Release lock before sending notification

        // Send notification if subscribed
        let subscribed = DEVICE_LIST_SUBSCRIBED.load(Ordering::Relaxed);
        info!("Device list subscription flag: {}", subscribed);
        
        if subscribed {
            let devices = self.get_all().await;
            let device_count = devices.len();
            info!("Building snapshot with {} devices", device_count);
            
            let response = protodongers::mux::MuxMsg::DevicesSnapshot(devices);
            HOST_RESPONSES.send(response).await;
            info!("Sent DevicesSnapshot notification with {} devices", device_count);
        } else {
            info!("Skipping snapshot notification - no subscription");
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

// Global references stored for access from control task
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

pub struct BleManager<C: trouble_host::Controller + 'static, P: PacketPool + 'static> {
    device_packets: &'static crate::ble::DevicePacketChannel,
    device_queues: &'static DeviceQueues,
    settings: &'static Settings,
    active_connections: &'static ActiveConnections,
    stack: &'static Stack<'static, C, P>,
    central: Option<Central<'static, C, P>>,
}

impl<C: trouble_host::Controller, P: PacketPool> BleManager<C, P> {
    pub fn new(
        device_packets: &'static crate::ble::DevicePacketChannel,
        device_queues: &'static DeviceQueues,
        settings: &'static Settings,
        active_connections: &'static ActiveConnections,
        stack: &'static Stack<'static, C, P>,
        central: Central<'static, C, P>,
    ) -> Self {
        defmt::info!("BleManager::new called");
        Self {
            device_packets,
            device_queues,
            settings,
            active_connections,
            stack,
            central: Some(central),
        }
    }

    // Other generic helper methods can go here.
}

// Provide `run` only when the controller supports the needed commands, without polluting the type.
impl<C, P> BleManager<C, P>
where
    C: trouble_host::Controller
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeSetScanParams>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeSetScanEnable>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeClearFilterAcceptList>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeAddDeviceToFilterAcceptList>
        + bt_hci::controller::ControllerCmdAsync<bt_hci::cmd::le::LeCreateConn>
        + bt_hci::controller::ControllerCmdAsync<bt_hci::cmd::le::LeSetPhy>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeReadPhy>,
    P: PacketPool,
{
    /// Run the BLE central role
    pub async fn run(&mut self) -> ! {
        info!("Starting BLE central manager");

        loop {
            let mut targets = self.settings.get_scan_targets().await;
            let pairing = crate::pairing::is_active();
            info!("Scan targets: {} (pairing={})", targets.len(), pairing);

            // Discover first seen device during pairing if none configured
            if pairing && targets.is_empty() {
                if let Some(addr) = scan_first_target::<C, P>(&mut self.central).await {
                    let _ = self.settings.add_scan_target(addr).await;
                    targets = self.settings.get_scan_targets().await;
                }
            }
            for addr in targets.iter() {
                // Skip if already connected
                if self.active_connections.contains(addr).await {
                    continue;
                }

                info!("Attempting connection to {:02x}", *addr);
                if let Err(e) = connect_to_device::<C, P>(self, BdAddr::new(*addr)).await {
                    error!("Connect attempt failed for {:02x}: {:?}", *addr, e);
                }
            }

            // Wait up to 2s before next cycle, but wake immediately if a restart is signalled
            let _ = embassy_time::with_timeout(Duration::from_secs(2), RESTART_CENTRAL.wait()).await;
        }
    }
}

// Helper free function to keep `run()` generic without long bounds.
async fn connect_to_device<C, P>(this: &mut BleManager<C, P>, target: BdAddr) -> Result<(), ()>
where
    C: trouble_host::Controller
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeClearFilterAcceptList>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeAddDeviceToFilterAcceptList>
        + bt_hci::controller::ControllerCmdAsync<bt_hci::cmd::le::LeCreateConn>
        + bt_hci::controller::ControllerCmdAsync<bt_hci::cmd::le::LeSetPhy>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeReadPhy>,
    P: PacketPool,
{
    info!("Connecting to device {:02x}...", target);

    let Some(central) = this.central.as_mut() else {
        error!("No central available to connect");
        return Err(());
    };

    let config = ConnectConfig {
        connect_params: ConnectParams {
            min_connection_interval: Duration::from_micros(7500),
            max_connection_interval: Duration::from_micros(15000),
            max_latency: 0,
            min_event_length: Duration::from_micros(0),
            max_event_length: Duration::from_micros(0),
            supervision_timeout: Duration::from_secs(10),
        },
        scan_config: ScanConfig {
            filter_accept_list: &[(AddrKind::RANDOM, &target)],
            phys: PhySet::M1,
            ..Default::default()
        },
    };
    info!("connect try kind=RANDOM {:02x}", target);

    // Try to connect with a timeout
    let connect_result =
        embassy_time::with_timeout(Duration::from_secs(10), central.connect(&config)).await;

    match connect_result {
        Ok(Ok(conn)) => {
            info!("Connected to {:02x}", target);

            // Check if we have an existing bond
            let has_bond = this.settings.get_bond(target).await.is_some();
            let _ = conn.set_bondable(true); // Always bondable for pairing and reconnection

            // If we don't have a bond, we need to pair first before creating L2CAP channel
            if !has_bond {
                info!("No bond found, initiating pairing for {:02x}", target);

                // Request security to initiate pairing (with NoInputNoOutput, this uses "Just Works")
                if let Err(e) = conn.request_security() {
                    error!("Failed to request security: {:?}", defmt::Debug2Format(&e));
                    return Err(());
                }

                // Wait for pairing to complete (with timeout)
                let pairing_timeout = Duration::from_secs(10);
                let pairing_result = embassy_time::with_timeout(pairing_timeout, async {
                    loop {
                        match conn.next().await {
                            trouble_host::prelude::ConnectionEvent::PairingComplete {
                                security_level,
                                bond,
                            } => {
                                info!("Pairing complete: {:?}", security_level);
                                if let Some(bi) = bond {
                                    let bd = crate::ble::security::bonddata_from_info(&bi);
                                    info!("Storing bond for {:02x}", bd.bd_addr);
                                    crate::ble::security::BOND_TO_STORE.signal(bd);
                                }
                                return Ok(());
                            }
                            trouble_host::prelude::ConnectionEvent::PairingFailed(err) => {
                                error!("Pairing failed: {:?}", err);
                                return Err(());
                            }
                            trouble_host::prelude::ConnectionEvent::Disconnected { reason } => {
                                error!("Disconnected during pairing: {:?}", reason);
                                return Err(());
                            }
                            _ => {
                                // Ignore other events during pairing
                            }
                        }
                    }
                })
                .await;

                match pairing_result {
                    Ok(Ok(())) => {
                        info!("Pairing successful, proceeding with L2CAP");
                    }
                    Ok(Err(())) => {
                        error!("Pairing failed or disconnected");
                        return Err(());
                    }
                    Err(_) => {
                        error!("Pairing timeout");
                        return Err(());
                    }
                }
            } else {
                info!("Using existing bond for {:02x}", target);
                // Request security to activate encryption using the stored bond
                if let Err(e) = conn.request_security() {
                    error!(
                        "Failed to request security for bonded device: {:?}",
                        defmt::Debug2Format(&e)
                    );
                    return Err(());
                }

                // Wait for encryption to be established (with timeout)
                let security_timeout = Duration::from_secs(10);
                let security_result = embassy_time::with_timeout(security_timeout, async {
                    loop {
                        match conn.next().await {
                            trouble_host::prelude::ConnectionEvent::PairingComplete {
                                security_level,
                                ..
                            } => {
                                info!("Encryption established: {:?}", security_level);
                                return Ok(());
                            }
                            trouble_host::prelude::ConnectionEvent::PairingFailed(err) => {
                                error!("Encryption failed: {:?}", err);
                                return Err(());
                            }
                            trouble_host::prelude::ConnectionEvent::Disconnected { reason } => {
                                error!("Disconnected during encryption: {:?}", reason);
                                return Err(());
                            }
                            _ => {
                                // Ignore other events
                            }
                        }
                    }
                })
                .await;

                match security_result {
                    Ok(Ok(())) => {
                        info!("Encryption successful, proceeding with L2CAP");
                    }
                    Ok(Err(())) => {
                        error!("Encryption failed or disconnected");
                        return Err(());
                    }
                    Err(_) => {
                        error!("Encryption timeout");
                        return Err(());
                    }
                }
            }

            // Request 2M PHY
            match conn
                .set_phy(this.stack, trouble_host::prelude::PhyKind::Le2M)
                .await
            {
                Ok(()) => {
                    info!("Requested 2M PHY");
                }
                Err(e) => match e {
                    trouble_host::BleHostError::BleHost(kind) => {
                        error!("2M PHY not supported or change failed: {:?}", kind)
                    }
                    trouble_host::BleHostError::Controller(_) => {
                        error!("2M PHY change controller error")
                    }
                },
            }

            // Now create L2CAP channels with encryption in place
            // Create control channel first, then data channel (VM expects this order)
            const PAYLOAD_LEN: u16 = 1024;
            const L2CAP_MTU: u16 = 251;

            // Control channel config - higher credits for reliable control packet delivery
            let l2cap_control_config = L2capChannelConfig {
                mtu: Some(PAYLOAD_LEN - 6),
                mps: Some(L2CAP_MTU - 4),
                flow_policy: CreditFlowPolicy::Every(5),
                initial_credits: Some(16), // 16 credits for control packets
            };

            // Data channel config - lower credits to leave packet pool space for control
            let l2cap_data_config = L2capChannelConfig {
                mtu: Some(PAYLOAD_LEN - 6),
                mps: Some(L2CAP_MTU - 4),
                flow_policy: CreditFlowPolicy::Every(5),
                initial_credits: Some(8), // Reduced to 8 to prevent pool exhaustion
            };

            // Create control channel (PSM 0x0080)
            info!(
                "Setting up L2CAP control channel with PSM {:04x}",
                L2CAP_PSM_CONTROL
            );
            let mut l2cap_control_channel = match embassy_time::with_timeout(
                Duration::from_secs(10),
                L2capChannel::create(this.stack, &conn, L2CAP_PSM_CONTROL, &l2cap_control_config),
            )
            .await
            {
                Ok(Ok(ch)) => {
                    info!(
                        "L2CAP control channel created successfully for {:02x}",
                        target
                    );
                    ch
                }
                Ok(Err(e)) => {
                    error!(
                        "Failed to create L2CAP control channel: {:?}",
                        defmt::Debug2Format(&e)
                    );
                    return Err(());
                }
                Err(_) => {
                    error!("L2CAP control channel creation timeout for {:02x}", target);
                    return Err(());
                }
            };

            // Create data channel (PSM 0x0081)
            info!(
                "Setting up L2CAP data channel with PSM {:04x}",
                L2CAP_PSM_DATA
            );
            let mut l2cap_data_channel = match embassy_time::with_timeout(
                Duration::from_secs(10),
                L2capChannel::create(this.stack, &conn, L2CAP_PSM_DATA, &l2cap_data_config),
            )
            .await
            {
                Ok(Ok(ch)) => {
                    info!("L2CAP data channel created successfully for {:02x}", target);
                    ch
                }
                Ok(Err(e)) => {
                    error!(
                        "Failed to create L2CAP data channel: {:?}",
                        defmt::Debug2Format(&e)
                    );
                    return Err(());
                }
                Err(_) => {
                    error!("L2CAP data channel creation timeout for {:02x}", target);
                    return Err(());
                }
            };

            // Register device queue for outgoing packets
            let device_queue = match this.device_queues.register(&target.into_inner()).await {
                Some(queue) => queue,
                None => {
                    error!("Failed to register device queue for {:02x}", target);
                    return Err(());
                }
            };

            // Add to active connections
            if !this.active_connections.add(&target.into_inner()).await {
                error!("Failed to add device to active connections");
                this.device_queues.unregister(&target.into_inner()).await;
                return Err(());
            }

            // Split L2CAP channels into separate readers and writers
            let (mut control_writer, mut control_reader) = l2cap_control_channel.split();
            let (mut data_writer, mut data_reader) = l2cap_data_channel.split();

            // Cooperative shutdown signal so we can avoid select; when one path ends, all exit.
            let shutdown = Signal::<ThreadModeRawMutex, ()>::new();

            // Build sub-futures
            let tx_fut = async {
                let mut tx_buf = [0u8; 1024];
                loop {
                    // Fast shutdown check
                    if embassy_time::with_timeout(Duration::from_millis(0), shutdown.wait()).await.is_ok() {
                        return;
                    }
                    let device_pkt = match device_queue.try_receive() {
                        Ok(pkt) => pkt,
                        Err(_) => {
                            // Bound idle to yield to other tasks
                            Timer::after(Duration::from_micros(200)).await;
                            continue;
                        }
                    };
                    let is_control = is_control_packet(&device_pkt.pkt);
                    if let Ok(data) = postcard::to_slice(&device_pkt.pkt, &mut tx_buf) {
                        let writer = if is_control { &mut control_writer } else { &mut data_writer };
                        let start = Instant::now();
                        match embassy_time::with_timeout(Duration::from_secs(10), writer.send(this.stack, data)).await {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => {
                                error!("L2CAP send failed: {:?}", defmt::Debug2Format(&e));
                                shutdown.signal(());
                                return;
                            }
                            Err(_) => {
                                warn!("L2CAP send timeout; will retry next packet");
                            }
                        }
                        let elapsed = Instant::now() - start;
                        if elapsed >= Duration::from_millis(100) {
                            warn!("L2CAP send took {} ms", elapsed.as_millis());
                        }
                    } else {
                        error!("Failed to serialize packet");
                    }
                }
            };

            let ctrl_rx_fut = async {
                let mut rx_buf = [0u8; 1024];
                loop {
                    if embassy_time::with_timeout(Duration::from_millis(0), shutdown.wait()).await.is_ok() {
                        return;
                    }
                    match control_reader.receive(this.stack, &mut rx_buf).await {
                        Ok(len) => match postcard::from_bytes::<protodongers::Packet>(&rx_buf[..len]) {
                            Ok(pkt) => {
                                let device_packet = DevicePacket { dev: target.into_inner(), pkt };
                                this.device_packets.send(device_packet).await;
                            }
                            Err(_) => error!("Failed to deserialize L2CAP control packet"),
                        },
                        Err(e) => {
                            error!("L2CAP control receive error: {:?}", defmt::Debug2Format(&e));
                            shutdown.signal(());
                            return;
                        }
                    }
                }
            };

            let data_rx_fut = async {
                let mut rx_buf = [0u8; 1024];
                loop {
                    if embassy_time::with_timeout(Duration::from_millis(0), shutdown.wait()).await.is_ok() {
                        return;
                    }
                    match data_reader.receive(this.stack, &mut rx_buf).await {
                        Ok(len) => match postcard::from_bytes::<protodongers::Packet>(&rx_buf[..len]) {
                            Ok(pkt) => {
                                let device_packet = DevicePacket { dev: target.into_inner(), pkt };
                                if let Err(_) = this.device_packets.try_send(device_packet) {
                                    let c = DROPPED_DATA_TO_HOST.fetch_add(1, Ordering::AcqRel) + 1;
                                    if c % 64 == 0 {
                                        warn!("Dropped {} data packets to host (channel full)", c);
                                    }
                                }
                            }
                            Err(_) => error!("Failed to deserialize L2CAP data packet"),
                        },
                        Err(e) => {
                            error!("L2CAP data receive error: {:?}", defmt::Debug2Format(&e));
                            shutdown.signal(());
                            return;
                        }
                    }
                }
            };

            let conn_fut = async {
                loop {
                    match conn.next().await {
                        trouble_host::prelude::ConnectionEvent::Disconnected { reason } => {
                            let c = DISCONNECT_COUNT.fetch_add(1, Ordering::AcqRel) + 1;
                            info!("Disconnected: {:?} (total={})", reason, c);
                            shutdown.signal(());
                            return;
                        }
                        trouble_host::prelude::ConnectionEvent::PhyUpdated { tx_phy, rx_phy } => {
                            info!("PHY updated tx={:?} rx={:?}", tx_phy, rx_phy);
                        }
                        trouble_host::prelude::ConnectionEvent::PairingComplete { security_level, bond } => {
                            info!("Pairing complete: {:?}", security_level);
                            if let Some(bi) = bond {
                                let bd = crate::ble::security::bonddata_from_info(&bi);
                                info!("Storing bond for {:02x}", bd.bd_addr);
                                crate::ble::security::BOND_TO_STORE.signal(bd);
                            }
                        }
                        trouble_host::prelude::ConnectionEvent::PairingFailed(err) => {
                            error!("Pairing failed: {:?}", err);
                        }
                        _ => {}
                    }
                }
            };

            // Run all sub-futures; join waits for graceful shutdown of all after signal
            use embassy_futures::join::join;
            let _ = join(join(tx_fut, ctrl_rx_fut), join(data_rx_fut, conn_fut)).await;

            // Clean up connection state before returning
            info!(
                "Connection loop ending for device {:02x}, cleaning up",
                target
            );
            this.active_connections.remove(&target.into_inner()).await;
            this.device_queues.unregister(&target.into_inner()).await;
            return Ok(());
        }
        Ok(Err(e)) => {
            match &e {
                trouble_host::BleHostError::BleHost(kind) => {
                    error!("Connect host error {:?} for {:02x}", kind, target)
                }
                trouble_host::BleHostError::Controller(_) => {
                    error!("Connect controller error for {:02x}", target)
                }
            }
            Err(())
        }
        Err(_) => {
            error!("Connect timeout for {:02x}", target);
            Err(())
        }
    }
}

async fn scan_first_target<C, P>(
    central_opt: &mut Option<Central<'static, C, P>>,
) -> Option<[u8; 6]>
where
    C: trouble_host::Controller
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeSetScanParams>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeSetScanEnable>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeClearFilterAcceptList>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeAddDeviceToFilterAcceptList>,
    P: PacketPool,
{
    info!("Pairing: scanning for first device...");
    reset_pairing_capture();

    let central = match central_opt.take() {
        Some(c) => c,
        None => {
            error!("No central available for scanning");
            return None;
        }
    };
    let mut scanner = Scanner::new(central);

    let mut config = ScanConfig::default();
    config.active = true;

    let mut found: Option<[u8; 6]> = None;
    {
        let res = scanner.scan(&config).await;
        match res {
            Ok(session) => {
                let result =
                    embassy_time::with_timeout(Duration::from_secs(10), PAIRING_FOUND_ADDR.wait())
                        .await;
                drop(session);
                if let Ok(addr) = result {
                    info!("Found device during pairing: {:02x}", addr);
                    found = Some(addr);
                } else {
                    info!("Pairing scan timeout - no matching device found");
                }
            }
            Err(e) => match &e {
                trouble_host::BleHostError::BleHost(kind) => {
                    error!("Scan start host error: {:?}", kind)
                }
                trouble_host::BleHostError::Controller(_) => error!("Scan start controller error"),
            },
        }
    }
    let inner = scanner.into_inner();
    *central_opt = Some(inner);
    found
}

// (Removed legacy split tasks: central_tx_task, central_control_rx_task, central_data_rx_task, central_conn_task)
