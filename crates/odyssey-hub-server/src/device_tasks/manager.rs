mod bootloader_devices;
mod direct_usb;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bootloader_devices::BootloaderDevice;
use odyssey_hub_common::events::Event;
use odyssey_protocol::ProductId;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::time::sleep;
use tracing::{info, trace, warn};

use super::context::{
    DeviceAddress, DeviceHandles, DeviceOffsets, DeviceTaskContext, ScreenCalibrations,
    ShotDelayWatchers, ShotDelays,
};
use super::hub_manager::{HubManager, HubManagerInit};
use super::messages::Message;

/// Nordic Semiconductor's USB vendor id, which every Odyssey unit enumerates under.
const ODYSSEY_USB_VENDOR_ID: u16 = 0x1915;

/// Pause between USB scans.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// Pause before retrying when the USB bus cannot be enumerated at all.
const SCAN_FAILURE_RETRY_DELAY: Duration = Duration::from_secs(5);

/// How long an aborted hub manager gets to wind down before it is left alone.
const HUB_MANAGER_EXIT_GRACE: Duration = Duration::from_secs(5);

/// Top-level entry for the device manager tasks.
///
/// This function runs the USB device manager which is responsible for discovering
/// USB devices, creating per-device tasks, and managing hub/dongle multiplexing.
/// It never returns.
pub(crate) async fn device_tasks(
    message_channel: mpsc::Sender<Message>,
    screen_calibrations: ScreenCalibrations,
    device_offsets: DeviceOffsets,
    device_shot_delays: ShotDelays,
    shot_delay_watch: ShotDelayWatchers,
    _accessory_map: Arc<
        std::sync::Mutex<HashMap<[u8; 6], (odyssey_hub_common::accessory::AccessoryInfo, bool)>>,
    >,
    _accessory_map_sender: broadcast::Sender<odyssey_hub_common::accessory::AccessoryMap>,
    _accessory_info_receiver: tokio::sync::watch::Receiver<
        odyssey_hub_common::accessory::AccessoryInfoMap,
    >,
    event_sender: broadcast::Sender<Event>,
) -> Result<()> {
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
    UsbDeviceManager::new(context).run().await
}

/// A hub manager task and the USB device it manages.
struct RunningHub {
    info: nusb::DeviceInfo,
    task: tokio::task::JoinHandle<()>,
}

/// What one USB scan found, for deciding afterwards what disappeared.
#[derive(Default)]
struct PresentUsbDevices {
    /// Opaque USB ids of devices that are Odyssey units (connected or not).
    direct_usb_ids: HashSet<String>,
    /// Opaque USB ids of units sitting in their DFU bootloader.
    bootloader_ids: HashSet<String>,
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
    /// Opaque USB device id -> unit, for devices detected in DFU mode or with broken
    /// protocol. These are surfaced in the device list so the user can flash firmware
    /// even when normal communication is unavailable.
    bootloader_devices: HashMap<String, BootloaderDevice>,
}

impl UsbDeviceManager {
    fn new(context: DeviceTaskContext) -> Self {
        Self {
            context,
            device_handles: Arc::new(Mutex::new(HashMap::new())),
            hubs: Vec::new(),
            direct_usb_id_to_uuid: HashMap::new(),
            last_uuid_by_pid: HashMap::new(),
            bootloader_devices: HashMap::new(),
        }
    }

    async fn run(mut self) -> ! {
        loop {
            let devices = match nusb::list_devices().await {
                Ok(devices) => devices.collect::<Vec<_>>(),
                Err(e) => {
                    warn!("Failed to list USB devices: {}", e);
                    sleep(SCAN_FAILURE_RETRY_DELAY).await;
                    continue;
                }
            };
            self.scan(&devices).await;
            sleep(SCAN_INTERVAL).await;
        }
    }

    async fn scan(&mut self, devices: &[nusb::DeviceInfo]) {
        log_scan(devices);
        self.stop_hub_managers_of_removed_hubs(devices);
        self.forget_finished_handlers().await;

        let mut present = PresentUsbDevices::default();
        for dev_info in devices {
            if dev_info.vendor_id() != ODYSSEY_USB_VENDOR_ID {
                continue;
            }
            // Skip non-ATS devices
            let Some(product_id) = ProductId::from_u16(dev_info.product_id()) else {
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

        self.reconcile_direct_usb_disconnects(&present).await;
        self.reconcile_bootloader_disconnects(&present).await;
    }

    /// Abort the hub manager of every hub that is no longer plugged in (compared by
    /// `DeviceInfo::id()`).
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
            // Detached on purpose: this only waits for the aborted task to wind down so the
            // outcome can be logged; nothing depends on it.
            tokio::spawn(async move {
                hub.task.abort();
                match tokio::time::timeout(HUB_MANAGER_EXIT_GRACE, hub.task).await {
                    Ok(result) => info!("Hub manager task exited: {:?}", result),
                    Err(_) => info!("Timed out waiting for hub manager to exit"),
                }
            });
        }
    }

    /// Clean up finished device handlers before checking for new connections.
    /// This ensures stale entries don't block reconnecting devices (e.g. after DFU).
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
            // Task finished, remove from list so we can reconnect
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

        // Subscribe to device list changes
        if let Err(e) = hub.subscribe_device_list().await {
            tracing::error!("Failed to subscribe to device list: {}", e);
            return;
        }
        tracing::info!("Subscribed to device list changes");

        // Runs until the hub disconnects; `hubs` owns the task and notices it finishing.
        let task = HubManager::spawn(HubManagerInit {
            hub,
            hub_info: dev_info.clone(),
            context: self.context.clone(),
            device_handles: Arc::clone(&self.device_handles),
        });
        self.hubs.push(RunningHub {
            info: dev_info.clone(),
            task,
        });
    }
}

fn log_scan(devices: &[nusb::DeviceInfo]) {
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
