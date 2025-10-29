use defmt::{error, info, warn};
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::{Duration, TimeoutError, Timer, with_timeout};
use heapless::Vec;
use nrf_softdevice::Softdevice;
use nrf_softdevice::ble::{Connection, SecurityMode, central, l2cap};
use protodongers::hub::{DevicePacket, MAX_DEVICES};
use static_cell::StaticCell;

use crate::ble::L2capPacket;
use crate::ble::security::DongleSecurityHandler;
use crate::storage::Settings;

// L2CAP PSM (Protocol Service Multiplexer) for our custom protocol
const L2CAP_PSM: u16 = 0x0080; // Dynamic PSM in valid range

pub type Uuid = [u8; 6];

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
            // Get all scan targets
            let scan_targets = self.settings.get_scan_targets().await;

            if scan_targets.is_empty() {
                info!("No scan targets, waiting...");
                Timer::after(Duration::from_secs(5)).await;
                continue;
            }

            info!("Scanning for {} target device(s)...", scan_targets.len());

            // Configure scan parameters
            let config = central::ScanConfig {
                whitelist: None,
                ..Default::default()
            };

            // Start scanning - scan() takes a callback
            let scan_result = central::scan(sd, &config, |report| {
                let addr_bytes: [u8; 6] = report.peer_addr.addr;

                // Check if this device is in our scan targets
                for target_addr in scan_targets.iter() {
                    if &addr_bytes == target_addr {
                        info!("Found target device {:02x}", target_addr);
                        // Return Some to stop scanning
                        return Some(*target_addr);
                    }
                }

                None // Continue scanning
            })
            .await;

            match scan_result {
                Ok(device_uuid) => {
                    info!("Connecting to device {:02x}...", device_uuid);

                    // Try to connect
                    if let Err(e) = self.connect_to_device(sd, &device_uuid).await {
                        error!("Failed to connect: {:?}", e);
                    }
                }
                Err(e) => {
                    error!("Scan error: {:?}", e);
                }
            }

            // Wait before next scan cycle
            Timer::after(Duration::from_secs(5)).await;
        }
    }

    async fn connect_to_device(&mut self, sd: &Softdevice, device_uuid: &Uuid) -> Result<(), ()> {
        info!("Connecting to device {:02x}...", device_uuid);

        let config = central::ConnectConfig {
            scan_config: central::ScanConfig {
                whitelist: None,
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

        info!("Connected to device {:02x}", device_uuid);

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
            .register(*device_uuid, device_queue)
            .await
        {
            error!("Failed to register device queue");
            return Err(());
        }

        // Add to active connections
        if let Err(_) = self.active_connections.add(*device_uuid).await {
            warn!("Too many active connections");
            self.device_queues.unregister(device_uuid).await;
            return Err(());
        }

        // Initiate pairing/bonding - this is mandatory for security
        info!("Requesting pairing for device {:02x}...", device_uuid);
        if let Err(e) = conn.request_pairing() {
            error!("Pairing request failed: {:?} - disconnecting", e);
            self.active_connections.remove(device_uuid).await;
            return Err(());
        }

        // Wait for pairing/bonding to complete
        // The SecurityHandler callbacks (on_bonded, on_security_update) are called during this process
        info!("Pairing initiated, waiting for bonding to complete...");

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
                info!("Device {:02x} paired and bonded successfully", device_uuid);
            }
            Ok(Err(())) => {
                error!("Pairing failed for device {:02x}", device_uuid);
                self.active_connections.remove(device_uuid).await;
                self.device_queues.unregister(device_uuid).await;
                return Err(());
            }
            Err(TimeoutError) => {
                error!(
                    "Pairing timeout for device {:02x} - disconnecting",
                    device_uuid
                );
                self.active_connections.remove(device_uuid).await;
                self.device_queues.unregister(device_uuid).await;
                return Err(());
            }
        }

        // Handle the connection
        if let Err(e) = self
            .handle_device_connection(&conn, device_uuid, device_queue)
            .await
        {
            error!("Connection handler error for {:02x}: {:?}", device_uuid, e);
        }

        // Remove from active connections and unregister queue when done
        self.active_connections.remove(device_uuid).await;
        self.device_queues.unregister(device_uuid).await;

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
