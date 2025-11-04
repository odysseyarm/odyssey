use defmt::{error, info};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use protodongers::hub::{DevicePacket, MAX_DEVICES};
use static_cell::StaticCell;

use trouble_host::prelude::{
    AddrKind, BdAddr, Central, ConnectConfig, ConnectParams, CreditFlowPolicy, L2capChannel,
    L2capChannelConfig, PhySet, ScanConfig,
};
use trouble_host::scan::Scanner;
use trouble_host::{PacketPool, Stack};

use crate::storage::Settings;
use core::sync::atomic::{AtomicBool, Ordering};

// L2CAP PSM (Protocol Service Multiplexer) for our custom protocol
const L2CAP_PSM: u16 = 0x0080; // Dynamic PSM in valid range

// Pairing filter: Company Identifier and ATS product prefixes
pub const TARGET_COMPANY_ID: u16 = 1674; // 0x068A Raytac (example)
pub const PRODUCT_ID_PREFIXES: &[&[u8]] = &[b"ATS", b"ATS-LEG"];

pub type Uuid = [u8; 6];

// Pairing discovery signal and helpers (first seen device during pairing)
pub static PAIRING_FOUND_ADDR: Signal<ThreadModeRawMutex, [u8; 6]> = Signal::new();
static PAIRING_CAPTURED: AtomicBool = AtomicBool::new(false);

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
pub type DeviceQueue = Channel<ThreadModeRawMutex, DevicePacket, 32>;

pub struct DeviceQueues {
    queues: AsyncMutex<
        ThreadModeRawMutex,
        heapless::LinearMap<Uuid, &'static DeviceQueue, MAX_DEVICES>,
    >,
}

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

        // Use const {} block to initialize array of StaticCells
        static QUEUES: [StaticCell<DeviceQueue>; MAX_DEVICES] = {
            const INIT: StaticCell<DeviceQueue> = StaticCell::new();
            [INIT; MAX_DEVICES]
        };
        static NEXT_IDX: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        let idx = NEXT_IDX.fetch_add(1, Ordering::Relaxed);
        if idx >= MAX_DEVICES {
            return None;
        }
        let queue = QUEUES[idx].init(Channel::new());
        let _ = map.insert(*uuid, queue);
        Some(queue)
    }

    pub async fn get(&self, uuid: &Uuid) -> Option<&'static DeviceQueue> {
        let map = self.queues.lock().await;
        map.get(uuid).copied()
    }

    pub async fn get_queue(&self, uuid: &Uuid) -> Option<&'static DeviceQueue> {
        self.get(uuid).await
    }

    pub async fn unregister(&self, uuid: &Uuid) {
        let mut map = self.queues.lock().await;
        let _ = map.remove(uuid);
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
        map.insert(*uuid, ()).is_ok()
    }

    pub async fn remove(&self, uuid: &Uuid) {
        let mut map = self.connections.lock().await;
        let _ = map.remove(uuid);
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

            // Small delay before next cycle
            Timer::after(Duration::from_secs(2)).await;
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
            supervision_timeout: Duration::from_secs(5),
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

            // Now create L2CAP channel with encryption in place
            const PAYLOAD_LEN: u16 = 1024;
            const L2CAP_MTU: u16 = 251;
            info!(
                "Setting up L2CAP channel with PSM {:04x}, MTU {}",
                L2CAP_PSM, L2CAP_MTU
            );
            let l2cap_config = L2capChannelConfig {
                mtu: Some(PAYLOAD_LEN - 6),
                mps: Some(L2CAP_MTU - 4),
                flow_policy: CreditFlowPolicy::Every(10), // Replenish credits more frequently
                initial_credits: Some(500), // Larger initial credit pool for high throughput
            };

            let mut l2cap_channel = match embassy_time::with_timeout(
                Duration::from_secs(10),
                L2capChannel::create(this.stack, &conn, L2CAP_PSM, &l2cap_config),
            )
            .await
            {
                Ok(Ok(ch)) => {
                    info!("L2CAP channel created successfully for {:02x}", target);
                    ch
                }
                Ok(Err(e)) => {
                    error!(
                        "Failed to create L2CAP channel: {:?}",
                        defmt::Debug2Format(&e)
                    );
                    return Err(());
                }
                Err(_) => {
                    error!("L2CAP channel creation timeout for {:02x}", target);
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

            // Keep connection alive and forward packets
            let mut rx_buf = [0u8; 1024];
            let mut tx_buf = [0u8; 1024];

            'connection_loop: loop {
                // DRAIN all incoming L2CAP packets (device -> dongle -> USB)
                loop {
                    match embassy_time::with_timeout(
                        Duration::from_millis(0), // Zero timeout = non-blocking check
                        l2cap_channel.receive(this.stack, &mut rx_buf),
                    )
                    .await
                    {
                        Ok(Ok(len)) => {
                            match postcard::from_bytes::<protodongers::Packet>(&rx_buf[..len]) {
                                Ok(pkt) => {
                                    let device_packet = DevicePacket {
                                        dev: target.into_inner(),
                                        pkt,
                                    };
                                    this.device_packets.send(device_packet).await;
                                }
                                Err(_) => {
                                    error!("Failed to deserialize L2CAP packet");
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            error!("L2CAP receive error: {:?}", defmt::Debug2Format(&e));
                            this.active_connections.remove(&target.into_inner()).await;
                            this.device_queues.unregister(&target.into_inner()).await;
                            break 'connection_loop Ok(());
                        }
                        Err(_) => break, // Timeout - no more packets available
                    }
                }

                // DRAIN all outgoing packets (USB -> dongle -> device)
                while let Ok(device_pkt) = device_queue.try_receive() {
                    info!("TX drain: sending packet to device");
                    match postcard::to_slice(&device_pkt.pkt, &mut tx_buf) {
                        Ok(data) => {
                            match embassy_time::with_timeout(
                                Duration::from_secs(5),
                                l2cap_channel.send(this.stack, data),
                            )
                            .await
                            {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) => {
                                    error!("L2CAP send failed: {:?}", defmt::Debug2Format(&e));
                                    this.active_connections.remove(&target.into_inner()).await;
                                    this.device_queues.unregister(&target.into_inner()).await;
                                    break 'connection_loop Ok(());
                                }
                                Err(_) => {
                                    error!("L2CAP send timeout");
                                    this.active_connections.remove(&target.into_inner()).await;
                                    this.device_queues.unregister(&target.into_inner()).await;
                                    break 'connection_loop Ok(());
                                }
                            }
                        }
                        Err(_) => {
                            error!("Failed to serialize packet");
                        }
                    }
                }

                // Wait for next event
                use embassy_futures::select::{Either3, select3};
                match select3(
                    device_queue.receive(),
                    embassy_time::with_timeout(
                        Duration::from_secs(30),
                        l2cap_channel.receive(this.stack, &mut rx_buf),
                    ),
                    conn.next(),
                )
                .await
                {
                    Either3::First(device_pkt) => {
                        match postcard::to_slice(&device_pkt.pkt, &mut tx_buf) {
                            Ok(data) => {
                                match embassy_time::with_timeout(
                                    Duration::from_secs(5),
                                    l2cap_channel.send(this.stack, data),
                                )
                                .await
                                {
                                    Ok(Ok(())) => {}
                                    Ok(Err(e)) => {
                                        error!("L2CAP send failed: {:?}", defmt::Debug2Format(&e));
                                        this.active_connections.remove(&target.into_inner()).await;
                                        this.device_queues.unregister(&target.into_inner()).await;
                                        break Ok(());
                                    }
                                    Err(_) => {
                                        error!("L2CAP send timeout");
                                        this.active_connections.remove(&target.into_inner()).await;
                                        this.device_queues.unregister(&target.into_inner()).await;
                                        break Ok(());
                                    }
                                }
                            }
                            Err(_) => {
                                error!("Failed to serialize packet");
                            }
                        }
                    }

                    Either3::Second(timeout_result) => match timeout_result {
                        Ok(Ok(len)) => {
                            match postcard::from_bytes::<protodongers::Packet>(&rx_buf[..len]) {
                                Ok(pkt) => {
                                    let device_packet = DevicePacket {
                                        dev: target.into_inner(),
                                        pkt,
                                    };
                                    this.device_packets.send(device_packet).await;
                                }
                                Err(_) => {
                                    error!("Failed to deserialize L2CAP packet");
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            error!("L2CAP receive error: {:?}", defmt::Debug2Format(&e));
                            this.active_connections.remove(&target.into_inner()).await;
                            this.device_queues.unregister(&target.into_inner()).await;
                            break Ok(());
                        }
                        Err(_) => {
                            error!("L2CAP receive timeout (30s) - connection may be stalled");
                            this.active_connections.remove(&target.into_inner()).await;
                            this.device_queues.unregister(&target.into_inner()).await;
                            break Ok(());
                        }
                    },

                    Either3::Third(event) => match event {
                        trouble_host::prelude::ConnectionEvent::Disconnected { reason } => {
                            info!("Disconnected: {:?}", reason);
                            this.active_connections.remove(&target.into_inner()).await;
                            this.device_queues.unregister(&target.into_inner()).await;
                            break Ok(());
                        }
                        trouble_host::prelude::ConnectionEvent::PhyUpdated { tx_phy, rx_phy } => {
                            info!("PHY updated tx={:?} rx={:?}", tx_phy, rx_phy);
                        }
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
                        }
                        trouble_host::prelude::ConnectionEvent::PairingFailed(err) => {
                            error!("Pairing failed: {:?}", err);
                        }
                        _ => {}
                    },
                }
            }
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
