mod bootloader_devices;
mod direct_usb;

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use nusb::hotplug::HotplugWatch;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::Event;
use odyssey_protocol::ProductId;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio::time::sleep;
use tracing::{debug, info, trace, warn};

use super::context::{
    DeviceAddress, DeviceHandles, DeviceOffsets, DeviceTaskContext, ScreenCalibrations,
    ShotDelayWatchers, ShotDelays,
};
use super::hub_manager::{HubManager, HubManagerInit};
use super::messages::Message;

/// Nordic Semiconductor's USB vendor id, which every Odyssey unit enumerates under.
const ODYSSEY_USB_VENDOR_ID: u16 = 0x1915;

/// Pause between USB scans. A plug or unplug the OS reports starts a scan at once.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// Pause before retrying when the USB bus cannot be enumerated at all.
const SCAN_FAILURE_RETRY_DELAY: Duration = Duration::from_secs(5);

/// What a simulator build adds to device discovery.
#[cfg(feature = "sim")]
pub(crate) struct Simulation {
    pub(crate) simulator: Option<odyssey_sim::Simulator>,
    /// False leaves the physical USB bus alone.
    pub(crate) discovery: bool,
}

/// Runs the USB device manager: discovers USB devices, starts a handler task per unit and
/// a hub manager per dongle. Never returns.
pub(crate) async fn device_tasks(
    message_channel: mpsc::Sender<Message>,
    screen_calibrations: ScreenCalibrations,
    device_offsets: DeviceOffsets,
    device_shot_delays: ShotDelays,
    shot_delay_watch: ShotDelayWatchers,
    event_sender: broadcast::Sender<Event>,
    #[cfg(feature = "sim")] simulation: Simulation,
) -> Result<(), std::convert::Infallible> {
    tracing::info!("device_tasks starting");
    let context = DeviceTaskContext {
        message_channel,
        screen_calibrations,
        device_offsets,
        device_shot_delays,
        shot_delay_watch,
        event_sender,
        direct_usb_handler_uuids: Arc::new(Mutex::new(HashSet::new())),
        bond_names: Arc::new(Mutex::new(HashMap::new())),
    };
    let manager = UsbDeviceManager::new(context);
    #[cfg(feature = "sim")]
    let manager = manager.with_simulator(simulation);
    manager.run().await
}

/// A hub manager task and the USB device it manages.
struct RunningHub {
    info: nusb::DeviceInfo,
    task: tokio::task::JoinHandle<()>,
    shutdown: oneshot::Sender<()>,
}

/// What one USB scan found, for deciding afterwards what disappeared.
#[derive(Default)]
struct PresentUsbDevices {
    /// Opaque USB ids of devices that are Odyssey units (connected or not).
    direct_usb_ids: HashSet<String>,
    /// Opaque USB ids of DFU-only peripherals and units sitting in their bootloader.
    dfu_ids: HashSet<String>,
}

/// USB device manager: discovers USB devices, handles hubs, and spawns per-device handlers.
///
/// Every scan it starts a [`HubManager`] for each dongle, creates a handler for each unit
/// plugged in directly, tells handlers when their cable disappears, and surfaces units
/// stuck in the bootloader so they can still be flashed.
struct UsbDeviceManager {
    context: DeviceTaskContext,
    /// Handler tasks by UUID, with the channel that reaches each. Shared with the hub
    /// managers, which add handlers for units reached over Bluetooth.
    device_handles: DeviceHandles,
    hubs: Vec<RunningHub>,
    /// Opaque USB device id -> UUID, once a unit has been probed successfully.
    direct_usb_id_to_uuid: HashMap<String, DeviceAddress>,
    /// The last uuid seen for each product id, which is how a bootloader that
    /// reports a placeholder serial can still be matched to the device that just
    /// detached. See `migration`.
    last_uuid_by_pid: HashMap<u16, DeviceAddress>,
    /// USB ids of DFU-only peripherals and recovery devices, with their device-list entries.
    dfu_devices: HashMap<String, Device>,
    /// Devices in the last scan whose list was logged; the timer rescans every
    /// `SCAN_INTERVAL`, so an unchanged list is not logged again.
    logged_device_ids: Vec<nusb::DeviceId>,
    #[cfg(feature = "sim")]
    simulated: Option<SimulatedDevices>,
    /// False leaves the physical USB bus alone.
    #[cfg(feature = "sim")]
    discovery: bool,
}

#[cfg(feature = "sim")]
struct SimulatedDevices {
    simulator: odyssey_sim::Simulator,
    changes: tokio::sync::watch::Receiver<odyssey_sim::Snapshot>,
    /// The manager of each plugged dongle, with the plug generation it serves.
    dongles: HashMap<DeviceAddress, (u64, tokio::task::JoinHandle<()>, oneshot::Sender<()>)>,
    /// The plug generation last seen for each plugged unit.
    units: HashMap<DeviceAddress, u64>,
}

/// Whether a simulated device was plugged, unplugged or reset since the scan
/// that recorded `units` and `dongles`.
#[cfg(feature = "sim")]
fn simulated_changed<T>(
    units: &HashMap<DeviceAddress, u64>,
    dongles: &HashMap<DeviceAddress, (u64, T, oneshot::Sender<()>)>,
    snapshot: &odyssey_sim::Snapshot,
) -> bool {
    let plugged_units = snapshot.units.iter().filter(|unit| unit.usb);
    let plugged_dongles = snapshot.dongles.iter().filter(|dongle| dongle.usb);
    plugged_units.clone().count() != units.len()
        || plugged_dongles.clone().count() != dongles.len()
        || plugged_units
            .into_iter()
            .any(|unit| units.get(&unit.uuid) != Some(&unit.usb_generation))
        || plugged_dongles.into_iter().any(|dongle| {
            dongles
                .get(&dongle.uuid)
                .is_none_or(|(generation, ..)| *generation != dongle.usb_generation)
        })
}

impl UsbDeviceManager {
    fn new(context: DeviceTaskContext) -> Self {
        Self {
            context,
            device_handles: Arc::new(Mutex::new(HashMap::new())),
            hubs: Vec::new(),
            direct_usb_id_to_uuid: HashMap::new(),
            last_uuid_by_pid: HashMap::new(),
            dfu_devices: HashMap::new(),
            logged_device_ids: Vec::new(),
            #[cfg(feature = "sim")]
            simulated: None,
            #[cfg(feature = "sim")]
            discovery: true,
        }
    }

    #[cfg(feature = "sim")]
    fn with_simulator(mut self, simulation: Simulation) -> Self {
        self.simulated = simulation.simulator.map(|simulator| SimulatedDevices {
            changes: simulator.handle().subscribe(),
            simulator,
            dongles: HashMap::new(),
            units: HashMap::new(),
        });
        self.discovery = simulation.discovery;
        self
    }

    async fn run(mut self) -> ! {
        #[cfg(feature = "sim")]
        let discovery = self.discovery;
        #[cfg(not(feature = "sim"))]
        let discovery = true;
        // A unit that picks its own transport switches its reports to USB the moment a
        // host configures it and back to BLE when the cable goes, so the hub has to
        // follow within milliseconds rather than at the next periodic scan.
        let mut hotplug = match discovery.then(nusb::watch_devices) {
            Some(Ok(watch)) => Some(Box::pin(watch)),
            Some(Err(e)) => {
                warn!("USB hotplug events unavailable, scanning every {SCAN_INTERVAL:?} only: {e}");
                None
            }
            None => None,
        };
        loop {
            let devices = if discovery {
                match nusb::list_devices().await {
                    Ok(devices) => devices.collect::<Vec<_>>(),
                    Err(error) => {
                        warn!("Failed to list USB devices: {error}");
                        sleep(SCAN_FAILURE_RETRY_DELAY).await;
                        continue;
                    }
                }
            } else {
                Vec::new()
            };
            self.scan(&devices).await;
            tokio::select! {
                () = wait_for_next_scan(hotplug.as_mut()) => {}
                () = self.simulated_change() => {}
            }
        }
    }

    /// Resolves when a simulated device is plugged, unplugged or reset.
    async fn simulated_change(&mut self) {
        #[cfg(feature = "sim")]
        if let Some(simulated) = &mut self.simulated {
            let SimulatedDevices {
                changes,
                dongles,
                units,
                ..
            } = simulated;
            let changed = changes
                .wait_for(|snapshot| simulated_changed(units, dongles, snapshot))
                .await;
            // A stopped simulator has nothing more to report.
            if changed.is_ok() {
                return;
            }
        }
        std::future::pending::<()>().await;
    }

    async fn scan(&mut self, devices: &[nusb::DeviceInfo]) {
        self.log_scan(devices);
        self.stop_hub_managers_of_removed_hubs(devices);
        self.forget_finished_handlers().await;

        let mut present = PresentUsbDevices::default();
        for dev_info in devices {
            if dev_info.vendor_id() != ODYSSEY_USB_VENDOR_ID {
                continue;
            }
            let device_type =
                |pid| crate::firmware::device_type_from_vid_pid(dev_info.vendor_id(), pid);
            if device_type(dev_info.product_id()).or_else(|| {
                dev_info
                    .product_id()
                    .checked_sub(0x10)
                    .and_then(device_type)
            }) == Some("marker-control-board")
            {
                self.on_marker_control_board(dev_info, &mut present).await;
                continue;
            }
            // Not an ATS product.
            let Some(product_id) = ProductId::from_u16(dev_info.product_id()).or_else(|| {
                dev_info
                    .product_id()
                    .checked_sub(0x10)
                    .and_then(ProductId::from_u16)
            }) else {
                continue;
            };
            // ATS Lites in BLE mode are enumerated too, for DFU/pairing support: a unit is
            // connected with connect_usb_any_mode(), which works regardless of transport mode.
            if product_id == ProductId::Mux {
                self.on_hub_device(dev_info).await;
            } else {
                self.on_direct_usb_device(dev_info, product_id, &mut present)
                    .await;
            }
        }

        #[cfg(feature = "sim")]
        self.scan_simulated(&mut present).await;

        self.reconcile_direct_usb_disconnects(&present).await;
        self.reconcile_dfu_disconnects(&present).await;
    }

    /// Stop the hub manager of every hub that is no longer plugged in (compared by
    /// `DeviceInfo::id()`), allowing it to unregister its dongle and BLE devices.
    fn stop_hub_managers_of_removed_hubs(&mut self, devices: &[nusb::DeviceInfo]) {
        let hubs = std::mem::take(&mut self.hubs);
        for hub in hubs {
            let still_present = devices.iter().any(|device| {
                device.vendor_id() == ODYSSEY_USB_VENDOR_ID
                    && ProductId::from_u16(device.product_id()) == Some(ProductId::Mux)
                    && device.id() == hub.info.id()
            });
            if still_present {
                self.hubs.push(hub);
                continue;
            }
            // Do not abort: the manager must remove its dongle and detach its BLE events.
            // It finishes the current bounded device operation before handling shutdown.
            let _ = hub.shutdown.send(());
            tokio::spawn(async move {
                match hub.task.await {
                    Ok(()) => info!("Hub manager task exited"),
                    Err(error) => warn!("Hub manager task failed during shutdown: {error}"),
                }
            });
        }
    }

    /// Forget handlers whose task has finished, so a stale entry does not block a unit that
    /// reconnects (e.g. after DFU). Runs before each check for new connections.
    async fn forget_finished_handlers(&self) {
        let mut handles = self.device_handles.lock().await;
        let finished: Vec<DeviceAddress> = handles
            .iter()
            .filter(|(_, device)| device.task.is_finished())
            .map(|(uuid, _)| *uuid)
            .collect();
        for uuid in finished {
            info!("Device {:02x?} task finished, cleaning up", uuid);
            if let Some(device) = handles.remove(&uuid) {
                device.task.abort();
                info!("Removed finished device handler for {:02x?}", uuid);
            }
            self.context
                .direct_usb_handler_uuids
                .lock()
                .await
                .remove(&uuid);
        }
    }

    /// Start a hub manager for a dongle unless one is already running for it.
    async fn on_hub_device(&mut self, dev_info: &nusb::DeviceInfo) {
        let existing = self
            .hubs
            .iter()
            .position(|hub| hub.info.id() == dev_info.id());
        if let Some(index) = existing {
            if !self.hubs[index].task.is_finished() {
                trace!("Hub already being managed, skipping: {:?}", dev_info.id());
                return;
            }
            // The task finished; dropping it lets the dongle get a fresh manager.
            info!(
                "Hub manager task finished for {:?}, removing from list to allow reconnect",
                dev_info.id()
            );
            self.hubs.remove(index);
        }

        info!("Attempting to connect to hub/dongle device");
        let hub = match odyssey_usb::device::MuxDevice::connect_usb(dev_info.clone()).await {
            Ok(hub) => hub,
            Err(e) => {
                warn!("Failed to connect to USB hub device: {}", e);
                return;
            }
        };
        info!("Connected USB hub device: {:?}", dev_info);

        if let Err(e) = hub.subscribe_device_list().await {
            tracing::error!("Failed to subscribe to device list: {}", e);
            return;
        }
        tracing::info!("Subscribed to device list changes");

        // Runs until the hub disconnects; `hubs` owns the task and notices it finishing.
        let (shutdown, shutdown_rx) = oneshot::channel();
        let task = HubManager::spawn(HubManagerInit {
            hub,
            serial: dev_info.serial_number().unwrap_or("unknown").to_owned(),
            context: self.context.clone(),
            device_handles: Arc::clone(&self.device_handles),
            shutdown: shutdown_rx,
        });
        self.hubs.push(RunningHub {
            info: dev_info.clone(),
            task,
            shutdown,
        });
    }
    #[cfg(feature = "sim")]
    async fn scan_simulated(&mut self, present: &mut PresentUsbDevices) {
        let Some(mut simulated) = self.simulated.take() else {
            return;
        };
        let snapshot = simulated.changes.borrow_and_update().clone();
        let stale: Vec<DeviceAddress> = simulated
            .dongles
            .iter()
            .filter(|(uuid, (generation, task, _))| {
                task.is_finished()
                    || snapshot
                        .dongle(**uuid)
                        .is_none_or(|dongle| !dongle.usb || dongle.usb_generation != *generation)
            })
            .map(|(uuid, _)| *uuid)
            .collect();
        for uuid in stale {
            if let Some((_, task, shutdown)) = simulated.dongles.remove(&uuid) {
                let _ = shutdown.send(());
                if let Err(error) = task.await {
                    warn!("Simulated hub task failed during shutdown: {error}");
                }
            }
        }
        for dongle in snapshot.dongles.iter().filter(|dongle| dongle.usb) {
            if simulated.dongles.contains_key(&dongle.uuid) {
                continue;
            }
            let hub = simulated.simulator.connect_dongle(dongle.uuid);
            match hub.subscribe_device_list().await {
                Ok(()) => {
                    let (shutdown, shutdown_rx) = oneshot::channel();
                    let task = HubManager::spawn(HubManagerInit {
                        hub,
                        serial: format!("SIM-{}", odyssey_sim::uuid_text::format(&dongle.uuid)),
                        context: self.context.clone(),
                        device_handles: Arc::clone(&self.device_handles),
                        shutdown: shutdown_rx,
                    });
                    simulated
                        .dongles
                        .insert(dongle.uuid, (dongle.usb_generation, task, shutdown));
                }
                Err(error) => warn!("Failed to subscribe to simulated dongle: {error}"),
            }
        }
        let plugged = snapshot.units.iter().filter(|unit| unit.usb);
        for unit in plugged.clone() {
            let id = format!("sim:{}", odyssey_sim::uuid_text::format(&unit.uuid));
            if simulated.units.get(&unit.uuid) != Some(&unit.usb_generation) {
                // A watch receiver may coalesce an unplug/replug into one snapshot.
                // Force replacement of the old control channel without replacing its
                // handler, which may still be receiving BLE events for this UUID.
                self.direct_usb_id_to_uuid.remove(&id);
            }
            present.direct_usb_ids.insert(id.clone());
            let connected = match self.direct_usb_id_to_uuid.get(&id) {
                Some(uuid) => self
                    .device_handles
                    .lock()
                    .await
                    .get(uuid)
                    .is_some_and(|device| !device.task.is_finished()),
                None => false,
            };
            if !connected {
                self.on_simulated_unit(simulated.simulator.connect_unit_usb(unit.uuid), id)
                    .await;
            }
        }
        simulated.units = plugged
            .map(|unit| (unit.uuid, unit.usb_generation))
            .collect();
        self.simulated = Some(simulated);
    }
}

/// Sleep until the next periodic scan, or less if the OS reports a USB device arriving or
/// leaving.
async fn wait_for_next_scan(hotplug: Option<&mut Pin<Box<HotplugWatch>>>) {
    let Some(hotplug) = hotplug else {
        sleep(SCAN_INTERVAL).await;
        return;
    };
    tokio::select! {
        () = sleep(SCAN_INTERVAL) => {}
        event = hotplug.next() => debug!("USB hotplug: {event:?}"),
    }
}

impl UsbDeviceManager {
    fn log_scan(&mut self, devices: &[nusb::DeviceInfo]) {
        let ids: Vec<_> = devices.iter().map(nusb::DeviceInfo::id).collect();
        if ids == self.logged_device_ids {
            tracing::trace!("USB scan: {} device(s), unchanged", devices.len());
            return;
        }
        self.logged_device_ids = ids;
        tracing::info!("USB scan: {} device(s) enumerated", devices.len());
        for dev in devices {
            tracing::info!(
                "USB scan entry: id={:?} VID={:04x} PID={:04x} serial={:?}",
                dev.id(),
                dev.vendor_id(),
                dev.product_id(),
                dev.serial_number()
            );
            tracing::debug!("USB scan entry full: {:?}", dev);
        }
    }
}

#[cfg(all(test, feature = "sim"))]
mod tests {
    use arc_swap::ArcSwap;
    use odyssey_sim::{Command, DONGLE_UUID, Simulator, Target, UNIT_UUID, dongle_uuid, unit_uuid};
    use tokio::sync::RwLock;

    use super::*;

    async fn next(messages: &mut mpsc::Receiver<Message>) -> Message {
        tokio::time::timeout(Duration::from_secs(5), messages.recv())
            .await
            .expect("the manager reports within the scan interval")
            .expect("the manager keeps running")
    }

    #[tokio::test]
    async fn every_simulated_unit_and_dongle_is_managed_without_usb_discovery() {
        let simulator = Simulator::new();
        let control = simulator.handle();
        let (second_unit, second_dongle) = (unit_uuid(1), dongle_uuid(1));
        control
            .execute(Command::AddUnit { uuid: second_unit })
            .await
            .unwrap();
        control
            .execute(Command::AddDongle {
                uuid: second_dongle,
            })
            .await
            .unwrap();
        let (message_channel, mut messages) = mpsc::channel(64);
        let manager = tokio::spawn(device_tasks(
            message_channel,
            Arc::new(ArcSwap::from_pointee(Default::default())),
            Arc::new(Mutex::new(Default::default())),
            Arc::new(RwLock::new(Default::default())),
            Arc::new(RwLock::new(Default::default())),
            broadcast::channel(16).0,
            Simulation {
                simulator: Some(simulator),
                discovery: false,
            },
        ));

        for unit in [UNIT_UUID, second_unit] {
            control
                .execute_on(Target::unit(unit), Command::UnitUsb { plugged: true })
                .await
                .unwrap();
        }
        let mut units = HashSet::new();
        while units.len() < 2 {
            if let Message::Connect(device, _) = next(&mut messages).await {
                units.insert(device.uuid);
            }
        }
        assert_eq!(units, HashSet::from([UNIT_UUID, second_unit]));

        for dongle in [DONGLE_UUID, second_dongle] {
            control
                .execute_on(Target::dongle(dongle), Command::DongleUsb { plugged: true })
                .await
                .unwrap();
        }
        let mut dongles = HashSet::new();
        while dongles.len() < 2 {
            if let Message::DongleConnect(dongle, _) = next(&mut messages).await {
                dongles.insert(dongle.id);
            }
        }
        assert_eq!(
            dongles,
            HashSet::from(["SIM-53494d000002".to_owned(), "SIM-53494d000102".to_owned()])
        );

        control
            .execute(Command::RemoveUnit { uuid: second_unit })
            .await
            .unwrap();
        loop {
            if let Message::Disconnect(device) = next(&mut messages).await {
                assert_eq!(device.uuid, second_unit);
                break;
            }
        }
        manager.abort();
    }
}
