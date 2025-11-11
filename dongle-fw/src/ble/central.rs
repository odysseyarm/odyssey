use defmt::{error, info};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use protodongers::mux::{DevicePacket, MAX_DEVICES};
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
            let (control_writer, control_reader) = l2cap_control_channel.split();
            let (data_writer, data_reader) = l2cap_data_channel.split();

            // Run 4 independent tasks: control TX, control RX, data TX, data RX
            use embassy_futures::select::{Either4, select4};
            let connection_result = select4(
                central_tx_task(
                    this.stack,
                    control_writer,
                    data_writer,
                    &device_queue,
                    target,
                ),
                central_control_rx_task(this.stack, control_reader, &this.device_packets, target),
                central_data_rx_task(this.stack, data_reader, &this.device_packets, target),
                central_conn_task(&conn, this.active_connections, &this.device_queues, target),
            )
            .await;

            // One of the tasks ended (connection lost, error, or disconnection)
            // Clean up connection state before returning
            info!(
                "Connection tasks ending for device {:02x}, cleaning up",
                target
            );
            this.active_connections.remove(&target.into_inner()).await;
            this.device_queues.unregister(&target.into_inner()).await;

            match connection_result {
                Either4::First(_) => {
                    info!("TX task ended for device {:02x}", target);
                }
                Either4::Second(_) => {
                    info!("Control RX task ended for device {:02x}", target);
                }
                Either4::Third(_) => {
                    info!("Data RX task ended for device {:02x}", target);
                }
                Either4::Fourth(_) => {
                    info!("Connection task ended for device {:02x}", target);
                }
            }
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

// TX task: sends packets from device queue to L2CAP channels
async fn central_tx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a Stack<'a, C, P>,
    mut control_writer: trouble_host::l2cap::L2capChannelWriter<'a, P>,
    mut data_writer: trouble_host::l2cap::L2capChannelWriter<'a, P>,
    device_queue: &'a DeviceQueue,
    target: BdAddr,
) {
    let mut tx_buf = [0u8; 1024];

    loop {
        let device_pkt = device_queue.receive().await;
        let is_control = is_control_packet(&device_pkt.pkt);

        info!(
            "TX: sending {} packet (id={}) to device",
            if is_control { "CONTROL" } else { "DATA" },
            device_pkt.pkt.id
        );

        match postcard::to_slice(&device_pkt.pkt, &mut tx_buf) {
            Ok(data) => {
                let writer = if is_control {
                    &mut control_writer
                } else {
                    &mut data_writer
                };

                match embassy_time::with_timeout(Duration::from_secs(5), writer.send(stack, data))
                    .await
                {
                    Ok(Ok(())) => {
                        info!(
                            "TX: {} packet sent successfully",
                            if is_control { "CONTROL" } else { "DATA" }
                        );
                    }
                    Ok(Err(e)) => {
                        error!("L2CAP send failed: {:?}", defmt::Debug2Format(&e));
                        return;
                    }
                    Err(_) => {
                        error!("L2CAP send timeout");
                        return;
                    }
                }
            }
            Err(_) => {
                error!("Failed to serialize packet");
            }
        }
    }
}

// Control RX task: receives control packets from L2CAP
async fn central_control_rx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a Stack<'a, C, P>,
    mut reader: trouble_host::l2cap::L2capChannelReader<'a, P>,
    device_packets: &'static crate::ble::DevicePacketChannel,
    target: BdAddr,
) {
    let mut rx_buf = [0u8; 1024];

    loop {
        match reader.receive(stack, &mut rx_buf).await {
            Ok(len) => match postcard::from_bytes::<protodongers::Packet>(&rx_buf[..len]) {
                Ok(pkt) => {
                    let device_packet = DevicePacket {
                        dev: target.into_inner(),
                        pkt,
                    };
                    device_packets.send(device_packet).await;
                }
                Err(_) => {
                    error!("Failed to deserialize L2CAP control packet");
                }
            },
            Err(e) => {
                error!("L2CAP control receive error: {:?}", defmt::Debug2Format(&e));
                return;
            }
        }
    }
}

// Data RX task: receives data packets from L2CAP
async fn central_data_rx_task<'a, C: trouble_host::Controller, P: PacketPool>(
    stack: &'a Stack<'a, C, P>,
    mut reader: trouble_host::l2cap::L2capChannelReader<'a, P>,
    device_packets: &'static crate::ble::DevicePacketChannel,
    target: BdAddr,
) {
    let mut rx_buf = [0u8; 1024];

    loop {
        match reader.receive(stack, &mut rx_buf).await {
            Ok(len) => match postcard::from_bytes::<protodongers::Packet>(&rx_buf[..len]) {
                Ok(pkt) => {
                    let device_packet = DevicePacket {
                        dev: target.into_inner(),
                        pkt,
                    };
                    device_packets.send(device_packet).await;
                }
                Err(_) => {
                    error!("Failed to deserialize L2CAP data packet");
                }
            },
            Err(e) => {
                error!("L2CAP data receive error: {:?}", defmt::Debug2Format(&e));
                return;
            }
        }
    }
}

// Connection task: handles connection events and disconnections
async fn central_conn_task<P: PacketPool>(
    conn: &trouble_host::prelude::Connection<'_, P>,
    active_connections: &ActiveConnections,
    device_queues: &DeviceQueues,
    _target: BdAddr,
) {
    loop {
        match conn.next().await {
            trouble_host::prelude::ConnectionEvent::Disconnected { reason } => {
                info!("Disconnected: {:?}", reason);
                active_connections.remove(&_target.into_inner()).await;
                device_queues.unregister(&_target.into_inner()).await;
                return;
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
        }
    }
}
