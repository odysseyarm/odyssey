use defmt::{error, info, warn};
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::{Duration, TimeoutError, Timer, with_timeout};
use heapless::Vec;
use nrf_softdevice::Softdevice;
use nrf_softdevice::ble::{Connection, SecurityMode, central, l2cap};
use protodongers::hub::{DevicePacket, HubMsg, MAX_DEVICES};
// local AD parsing
use static_cell::StaticCell;

use crate::ble::L2capPacket;
use crate::ble::security::DongleSecurityHandler;
use crate::storage::Settings;

// L2CAP PSM (Protocol Service Multiplexer) for our custom protocol
const L2CAP_PSM: u16 = 0x0080; // Dynamic PSM in valid range
// Optional: set these to prefer manufacturer data matching (pre-connection filtering)
// Bluetooth SIG Company Identifier (16-bit). Set to Some(0xFFFF) when known.
// Bluetooth SIG Company Identifier (decimal). Raytac = 1674 (0x068A)
const COMPANY_ID: Option<u16> = Some(1674);
// Product ID prefixes immediately following company ID in MSD payload for ATS peripherals
const PRODUCT_ID_PREFIXES: &[&[u8]] = &[b"ATS", b"ATS-LEG"];

pub type Uuid = [u8; 6];

#[derive(Copy, Clone)]
struct ConnectTarget {
    bytes: [u8; 6],
    addr: nrf_softdevice::ble::Address,
}

/// Channel for outgoing packets (from device to USB)
pub type DevicePacketChannel = Channel<ThreadModeRawMutex, DevicePacket, 8>;

/// Per-device outgoing message queue
pub type DeviceQueue = Channel<ThreadModeRawMutex, protodongers::Packet, 8>;

/// Registry of per-device queues
pub struct DeviceQueues {
    queues: AsyncMutex<ThreadModeRawMutex, Vec<(Uuid, &'static DeviceQueue), MAX_DEVICES>>,
}

impl DeviceQueues {
    pub const fn new() -> Self {
        Self {
            queues: AsyncMutex::new(Vec::new()),
        }
    }

    /// Register a new device queue
    pub async fn register(&self, uuid: Uuid, queue: &'static DeviceQueue) -> Result<(), ()> {
        let mut queues = self.queues.lock().await;
        queues.push((uuid, queue)).map_err(|_| ())
    }

    /// Unregister a device queue
    pub async fn unregister(&self, uuid: &Uuid) {
        let mut queues = self.queues.lock().await;
        queues.retain(|(u, _)| u != uuid);
    }

    /// Get the queue for a specific device
    pub async fn get_queue(&self, uuid: &Uuid) -> Option<&'static DeviceQueue> {
        let queues = self.queues.lock().await;
        for (u, queue) in queues.iter() {
            if u == uuid {
                return Some(*queue);
            }
        }
        None
    }
}

pub static DEVICE_QUEUES: StaticCell<DeviceQueues> = StaticCell::new();

/// Pool of static device queues (one per potential device connection)
static DEVICE_QUEUE_POOL: [StaticCell<DeviceQueue>; MAX_DEVICES] = [
    StaticCell::new(),
    StaticCell::new(),
    StaticCell::new(),
    StaticCell::new(),
    StaticCell::new(),
    StaticCell::new(),
    StaticCell::new(),
];

/// Allocate a device queue from the pool
fn allocate_device_queue() -> Option<&'static DeviceQueue> {
    for queue_cell in DEVICE_QUEUE_POOL.iter() {
        if let Some(queue) = queue_cell.try_init(Channel::new()) {
            return Some(queue);
        }
    }
    None
}

/// Shared state for active connections (for HubMsg::DevicesSnapshot)
pub struct ActiveConnections {
    connections: AsyncMutex<ThreadModeRawMutex, Vec<Uuid, MAX_DEVICES>>,
}

impl ActiveConnections {
    pub const fn new() -> Self {
        Self {
            connections: AsyncMutex::new(Vec::new()),
        }
    }

    pub async fn add(&self, uuid: Uuid) -> Result<(), ()> {
        let mut conns = self.connections.lock().await;
        conns.push(uuid).map_err(|_| ())
    }

    pub async fn remove(&self, uuid: &Uuid) {
        let mut conns = self.connections.lock().await;
        conns.retain(|u| u != uuid);
    }

    pub async fn get_all(&self) -> Vec<Uuid, MAX_DEVICES> {
        let conns = self.connections.lock().await;
        conns.clone()
    }
}

pub static ACTIVE_CONNECTIONS: StaticCell<ActiveConnections> = StaticCell::new();

/// BLE Central Manager
pub struct BleManager {
    device_packets: &'static DevicePacketChannel,
    device_queues: &'static DeviceQueues,
    settings: &'static Settings,
    active_connections: &'static ActiveConnections,
    security_handler: &'static DongleSecurityHandler,
    l2cap: &'static l2cap::L2cap<L2capPacket>,
}

impl BleManager {
    pub fn new(
        device_packets: &'static DevicePacketChannel,
        device_queues: &'static DeviceQueues,
        settings: &'static Settings,
        active_connections: &'static ActiveConnections,
        security_handler: &'static DongleSecurityHandler,
        l2cap: &'static l2cap::L2cap<L2capPacket>,
    ) -> Self {
        Self {
            device_packets,
            device_queues,
            settings,
            active_connections,
            security_handler,
            l2cap,
        }
    }

    /// Run the BLE central role
    pub async fn run(&mut self, sd: &'static Softdevice) -> ! {
        info!("Starting BLE central manager");

        loop {
            // Get all scan targets (for non-pairing mode) and log pairing state
            let scan_targets = self.settings.get_scan_targets().await;
            info!(
                "Scanning (pairing={}, targets={})",
                crate::pairing::is_active(),
                scan_targets.len()
            );

            // Configure scan parameters
            let config = central::ScanConfig {
                active: true,
                whitelist: None,
                ..Default::default()
            };

            // Race scan against restart signal with timeout
            let scan_fut = central::scan(sd, &config, |report| {
                let addr_bytes: [u8; 6] = report.peer_addr.addr;
                let len = report.data.len as usize;
                let data = unsafe { core::slice::from_raw_parts(report.data.p_data, len) };

                // Evaluate pairing flag for each report to pick up runtime changes
                if crate::pairing::is_active() {
                    if adv_is_target(data) {
                        let addr = nrf_softdevice::ble::Address::from_raw(report.peer_addr);
                        return Some(ConnectTarget {
                            bytes: report.peer_addr.addr,
                            addr,
                        });
                    }
                    return None;
                } else {
                    for target_addr in scan_targets.iter() {
                        if &addr_bytes == target_addr {
                            let addr = nrf_softdevice::ble::Address::from_raw(report.peer_addr);
                            return Some(ConnectTarget {
                                bytes: *target_addr,
                                addr,
                            });
                        }
                    }
                    None
                }
            });

            let restart_fut = crate::pairing::wait_for_scan_restart();
            let timeout_fut = Timer::after(Duration::from_secs(10));

            // Use select3 to allow scan to be interrupted by pairing state changes or timeout
            match embassy_futures::select::select3(scan_fut, restart_fut, timeout_fut).await {
                embassy_futures::select::Either3::First(scan_result) => {
                    info!("Scan completed with result");
                    // Scan completed (found target or error)
                    match scan_result {
                        Ok(target) => {
                            info!("Connecting to device {:02x}...", target.bytes);

                            // Try to connect
                            match self.connect_to_device(sd, &target).await {
                                Ok(()) => {
                                    // Notify host of pairing result if pairing is still active now
                                    if crate::pairing::is_active() {
                                        crate::control::try_send(HubMsg::PairingResult(
                                            target.bytes,
                                        ));
                                        crate::pairing::cancel();
                                    }
                                }
                                Err(e) => error!("Failed to connect: {:?}", e),
                            }
                        }
                        Err(e) => {
                            error!("Scan error: {:?}", e);
                        }
                    }

                    // Wait before next scan cycle
                    Timer::after(Duration::from_secs(5)).await;
                }
                embassy_futures::select::Either3::Second(_) => {
                    info!(
                        "Pairing state changed, restarting scan (pairing={})",
                        crate::pairing::is_active()
                    );
                    // No delay - restart scan loop immediately to pick up new state
                }
                embassy_futures::select::Either3::Third(_) => {
                    info!(
                        "Scan timeout reached, restarting (pairing={})",
                        crate::pairing::is_active()
                    );
                    // Timeout - restart to ensure we pick up any state changes
                }
            }
        }
    }

    async fn connect_to_device(
        &mut self,
        sd: &Softdevice,
        target: &ConnectTarget,
    ) -> Result<(), ()> {
        info!("Connecting to device {:02x}...", target.bytes);

        let wl = [&target.addr];
        let config = central::ConnectConfig {
            scan_config: central::ScanConfig {
                whitelist: Some(&wl),
                ..Default::default()
            },
            conn_params: nrf_softdevice::raw::ble_gap_conn_params_t {
                min_conn_interval: 12, // 15ms
                max_conn_interval: 24, // 30ms
                slave_latency: 0,
                conn_sup_timeout: 200, // 2s
            },
        };

        let conn = match central::connect_with_security(sd, &config, self.security_handler).await {
            Ok(c) => c,
            Err(e) => {
                error!("Connection failed: {:?}", e);
                return Err(());
            }
        };

        info!("Connected to device {:02x}", target.bytes);

        // Allocate a dedicated queue for this device
        let device_queue = match allocate_device_queue() {
            Some(q) => q,
            None => {
                error!("No available device queues - max connections reached");
                return Err(());
            }
        };

        // Register the queue for this device
        if let Err(_) = self
            .device_queues
            .register(target.bytes, device_queue)
            .await
        {
            error!("Failed to register device queue");
            return Err(());
        }

        // Add to active connections
        if let Err(_) = self.active_connections.add(target.bytes).await {
            warn!("Too many active connections");
            self.device_queues.unregister(&target.bytes).await;
            return Err(());
        }

        // Initiate pairing/bonding - this is mandatory for security
        // Add small delay to let peripheral fully initialize
        info!(
            "Connection established for {:02x}, waiting 500ms before pairing...",
            target.bytes
        );
        Timer::after(Duration::from_millis(500)).await;

        // Log connection state before pairing
        let pre_pair_security = conn.security_mode();
        info!("Pre-pairing security mode: {:?}", pre_pair_security);

        info!("Requesting pairing for device {:02x}...", target.bytes);

        // Request pairing - our SecurityHandler's security_params() will provide LESC-enabled params
        if let Err(e) = conn.request_pairing() {
            error!("Pairing request failed: {:?} - disconnecting", e);
            self.active_connections.remove(&target.bytes).await;
            return Err(());
        }

        info!("Pairing request sent (LESC enabled via SecurityHandler)");

        // Wait for pairing/bonding to complete
        // The SecurityHandler callbacks (on_bonded, on_security_update) are called during this process
        info!("Pairing request sent, waiting for bonding to complete...");

        let wait_for_pairing = async {
            loop {
                Timer::after(Duration::from_millis(100)).await;
                let sec_mode = conn.security_mode();
                match sec_mode {
                    SecurityMode::Open | SecurityMode::NoAccess => {
                        // Still waiting for security to be established
                        continue;
                    }
                    SecurityMode::JustWorks | SecurityMode::Mitm | SecurityMode::LescMitm => {
                        // Pairing complete!
                        info!("Pairing successful - security mode: {:?}", sec_mode);
                        return Ok(());
                    }
                    _ => {
                        // Signed modes or other - consider successful
                        info!("Security established - mode: {:?}", sec_mode);
                        return Ok(());
                    }
                }
            }
        };

        // Give it 30 seconds to pair (generous timeout for user interaction if needed)
        match with_timeout(Duration::from_secs(30), wait_for_pairing).await {
            Ok(Ok(())) => {
                info!("Device {:02x} paired and bonded successfully", target.bytes);
            }
            Ok(Err(())) => {
                error!("Pairing failed for device {:02x}", target.bytes);
                self.active_connections.remove(&target.bytes).await;
                self.device_queues.unregister(&target.bytes).await;
                return Err(());
            }
            Err(TimeoutError) => {
                error!(
                    "Pairing timeout for device {:02x} - disconnecting",
                    target.bytes
                );
                self.active_connections.remove(&target.bytes).await;
                self.device_queues.unregister(&target.bytes).await;
                return Err(());
            }
        }

        // Handle the connection
        if let Err(e) = self
            .handle_device_connection(&conn, &target.bytes, device_queue)
            .await
        {
            error!("Connection handler error for {:02x}: {:?}", target.bytes, e);
        }

        // Remove from active connections and unregister queue when done
        self.active_connections.remove(&target.bytes).await;
        self.device_queues.unregister(&target.bytes).await;

        Ok(())
    }

    async fn handle_device_connection(
        &self,
        conn: &Connection,
        device_uuid: &Uuid,
        device_queue: &'static DeviceQueue,
    ) -> Result<(), ()> {
        info!("Handling connection for device {:02x}", device_uuid);

        // Setup L2CAP channel
        info!("Setting up L2CAP channel with PSM 0x{:04x}...", L2CAP_PSM);

        let config = l2cap::Config {
            credits: 8, // Flow control credits
        };

        let channel = match self.l2cap.setup(conn, &config, L2CAP_PSM).await {
            Ok(ch) => {
                info!("L2CAP channel established for device {:02x}", device_uuid);
                ch
            }
            Err(_e) => {
                error!("L2CAP setup failed");
                return Err(());
            }
        };

        // Handle bidirectional data transfer over L2CAP
        loop {
            // Use select to handle both TX and RX concurrently
            match select(
                // Wait for incoming packet from device
                channel.rx(),
                // Wait for outgoing message from this device's queue
                device_queue.receive(),
            )
            .await
            {
                // Incoming: Device -> USB
                Either::First(rx_result) => {
                    match rx_result {
                        Ok(packet) => {
                            info!("Received L2CAP packet from device {:02x}", device_uuid);

                            // Deserialize packet using postcard
                            match postcard::from_bytes::<DevicePacket>(packet.as_slice()) {
                                Ok(device_packet) => {
                                    // Send to USB (infallible - waits if queue is full)
                                    self.device_packets.send(device_packet).await;
                                }
                                Err(_e) => {
                                    error!("Failed to deserialize packet from device");
                                }
                            }
                        }
                        Err(_e) => {
                            error!("L2CAP RX error - disconnecting");
                            return Err(()); // Disconnect on RX error
                        }
                    }
                }

                // Outgoing: USB -> Device
                Either::Second(packet) => {
                    info!("Sending packet to device {:02x}", device_uuid);

                    // Serialize packet using postcard
                    let mut buf = [0u8; 512];
                    match postcard::to_slice(&packet, &mut buf) {
                        Ok(data) => {
                            // Create L2CAP packet
                            match L2capPacket::from_slice(data) {
                                Some(packet) => {
                                    if let Err(_e) = channel.tx(packet).await {
                                        error!("L2CAP TX failed");
                                        return Err(()); // Disconnect on TX error
                                    }
                                }
                                None => {
                                    error!("Failed to create L2CAP packet");
                                }
                            }
                        }
                        Err(_e) => {
                            error!("Failed to serialize packet");
                        }
                    }
                }
            }
        }
    }
}

fn adv_is_target(payload: &[u8]) -> bool {
    if let Some(cid) = COMPANY_ID {
        if adv_has_msd(payload, cid) {
            return true;
        }
    }
    false
}

fn adv_has_msd(payload: &[u8], company_id: u16) -> bool {
    let mut i = 0;
    while i < payload.len() {
        let len = payload[i] as usize;
        if len == 0 {
            break;
        }
        if i + 1 + len > payload.len() {
            break;
        }
        let ty = payload[i + 1];
        if ty == 0xFF {
            let data = &payload[(i + 2)..(i + 1 + len)];
            if data.len() >= 2 {
                let cid = u16::from_le_bytes([data[0], data[1]]);
                if cid == company_id {
                    if PRODUCT_ID_PREFIXES.is_empty() {
                        return true;
                    }
                    for prefix in PRODUCT_ID_PREFIXES {
                        if data.len() >= 2 + prefix.len() && &data[2..(2 + prefix.len())] == *prefix
                        {
                            info!(
                                "scan: MSD matched cid=0x{:04x} prefix={:?}",
                                cid,
                                defmt::Debug2Format(prefix)
                            );
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
