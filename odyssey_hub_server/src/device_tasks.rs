use ahrs::Ahrs;
use anyhow::Result;
use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use ats_common::{ros_opencv_intrinsics_type_convert, ScreenCalibration};
use ats_usb::device::VmDevice;
use ats_usb::packets::vm::{
    AccelReport, CombinedMarkersReport, ConfigKind, GeneralConfig, ImpactReport, Packet,
    PacketData, PocMarkersReport, VendorData,
};
use nalgebra::{
    Isometry3, Matrix3, Matrix3x1, Point2, Point3, Rotation3, Translation3, UnitQuaternion,
    UnitVector3, Vector2, Vector3,
};
use odyssey_hub_common::device::{Device, DeviceCapabilities};
use odyssey_hub_common::events::{
    AccelerometerEvent, DeviceEvent, DeviceEventKind, Event, ImpactEvent, Pose, TrackingEvent,
};
use std::collections::HashMap;

use futures::stream::{self, BoxStream, SelectAll, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch, Mutex, RwLock};
use tokio::time::sleep;
use tracing::{debug, info, trace, warn};

/// Events from the hub manager's merged stream
enum HubEvent {
    Snapshot(Result<heapless::Vec<[u8; 6], 3>>),
}

enum DeviceStreamEvent {
    CombinedMarkers(CombinedMarkersReport),
    PocMarkers(PocMarkersReport),
    Accel(AccelReport),
    Impact(ImpactReport),
    Vendor(u8, VendorData),
}

/// Events from the device handler's merged stream
// ats_cv foveated helpers
use ats_cv::{
    foveated::FoveatedAimpointState, helpers as ats_helpers, to_normalized_image_coordinates,
    undistort_points,
};
use nalgebra::Point2 as NaPoint2;
use opencv_ros_camera::RosOpenCvIntrinsics;

// per-handler foveated state and last-timestamp are local to each device handler now.

/// Control messages sent into per-device task
pub enum DeviceTaskMessage {
    ResetZero,
    SaveZero,
    /// Send a vendor packet to the device. The Vec bytes will be truncated to 98 bytes.
    WriteVendor(u8, Vec<u8>),
    /// Set the shot delay (milliseconds) for this device. Server may send this
    /// to apply shot delay live to the running device task.
    SetShotDelay(u16),
    Zero(
        nalgebra::Translation3<f32>,
        nalgebra::Point2<f32>,
        Option<TrackingEvent>,
    ),
    ClearZero,
    /// Read a sensor register. Reply channel returns the register value or an error.
    ReadRegister {
        port: u8,
        bank: u8,
        address: u8,
        reply: tokio::sync::oneshot::Sender<Result<u8, String>>,
    },
    /// Write a sensor register. Reply channel returns success or an error.
    WriteRegister {
        port: u8,
        bank: u8,
        address: u8,
        data: u8,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// Read a device config value (impact threshold, suppress ms, etc.)
    ReadConfig {
        kind: u8,
        reply: tokio::sync::oneshot::Sender<Result<u8, String>>,
    },
    /// Write a device config value.
    WriteConfig {
        kind: u8,
        value: u8,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// Persist current settings to device flash. Reply channel returns success or an error.
    FlashSettings {
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// Start BLE pairing mode. Reply returns Ok(()) when pairing mode is entered.
    /// The actual pairing result arrives as a PairingResult event.
    StartPairing {
        timeout_ms: u32,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// Cancel an in-progress pairing operation.
    CancelPairing {
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// Clear bond(s) on the device. For ATS: clears single bond. For dongle: clears all bonds.
    ClearBond {
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// Set the transport mode (USB or BLE). The device will persist this and apply after reboot.
    SetTransportMode {
        usb_mode: bool,
        reply: tokio::sync::oneshot::Sender<Result<bool, String>>,
    },
    /// Add an events device to enable sensor streams (for merging USB direct + BLE mux)
    AddEventsDevice(VmDevice),
    /// Add a control device to enable control operations (for merging BLE mux + USB direct BLE mode)
    AddControlDevice(VmDevice),
    /// Remove control device when direct USB disconnects
    RemoveControlDevice,
    /// Remove events device when BLE mux disconnects
    RemoveEventsDevice,
}

/// Device connection channels
pub struct DeviceChannels {
    /// Commands channel for device operations (register read/write, zero, etc.)
    pub commands: Option<mpsc::Sender<DeviceTaskMessage>>,
}

/// Commands that can be sent to a dongle (hub manager task).
pub enum DongleTaskMessage {
    StartPairing {
        timeout_ms: u32,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    CancelPairing {
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    ClearBonds {
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
}

/// Dongle metadata (for dongle list).
#[derive(Clone, Debug)]
pub struct DongleInfo {
    pub id: String,                         // USB serial number
    pub protocol_version: [u16; 3],         // mux endpoint protocol version
    pub control_protocol_version: [u16; 3], // control endpoint protocol version
    pub firmware_version: [u16; 3],
    pub connected_devices: Vec<[u8; 6]>, // BLE addresses of connected ATS devices
    pub bonded_devices: Vec<[u8; 6]>,    // BLE addresses of bonded ATS devices
}

fn semver_at_least(version: [u16; 3], required: [u16; 3]) -> bool {
    version[0] > required[0]
        || (version[0] == required[0] && version[1] > required[1])
        || (version[0] == required[0] && version[1] == required[1] && version[2] >= required[2])
}

/// Messages emitted by device_tasks to the rest of the system.
pub enum Message {
    Connect(Device, DeviceChannels),
    Disconnect(Device),
    UpdateDevice(Device), // Update device metadata (e.g., capabilities changed)
    DongleConnect(DongleInfo, mpsc::Sender<DongleTaskMessage>),
    DongleUpdate(DongleInfo),
    DongleDisconnect(String), // dongle id
    #[allow(dead_code)]
    Event(Event),
}

/// Top-level entry for spawning device manager tasks.
///
/// This function spawns the USB device manager which is responsible for discovering
/// USB devices, creating per-device tasks, and managing hub/dongle multiplexing.
pub async fn device_tasks(
    message_channel: mpsc::Sender<Message>,
    screen_calibrations: Arc<
        ArcSwap<
            ArrayVec<(u8, ScreenCalibration<f32>), { (ats_common::MAX_SCREEN_ID + 1) as usize }>,
        >,
    >,
    device_offsets: Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
    device_shot_delays: Arc<RwLock<HashMap<[u8; 6], u16>>>,
    shot_delay_watch: Arc<RwLock<HashMap<[u8; 6], watch::Sender<u32>>>>,
    _accessory_map: Arc<
        std::sync::Mutex<HashMap<[u8; 6], (odyssey_hub_common::accessory::AccessoryInfo, bool)>>,
    >,
    _accessory_map_sender: broadcast::Sender<odyssey_hub_common::accessory::AccessoryMap>,
    _accessory_info_receiver: watch::Receiver<odyssey_hub_common::accessory::AccessoryInfoMap>,
    event_sender: broadcast::Sender<Event>,
) -> Result<()> {
    // Simply run the usb_device_manager - this should never return
    tracing::info!("device_tasks starting");
    usb_device_manager(
        message_channel.clone(),
        screen_calibrations.clone(),
        device_offsets.clone(),
        device_shot_delays.clone(),
        shot_delay_watch.clone(),
        event_sender.clone(),
    )
    .await;
    tracing::error!("usb_device_manager returned unexpectedly!");
    Ok(())
}

/// USB device manager: discovers USB devices, handles hubs, and spawns per-device handlers.
async fn usb_device_manager(
    message_channel: mpsc::Sender<Message>,
    screen_calibrations: Arc<
        ArcSwap<
            ArrayVec<(u8, ScreenCalibration<f32>), { (ats_common::MAX_SCREEN_ID + 1) as usize }>,
        >,
    >,
    device_offsets: Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
    device_shot_delays: Arc<RwLock<HashMap<[u8; 6], u16>>>,
    shot_delay_watch: Arc<RwLock<HashMap<[u8; 6], watch::Sender<u32>>>>,
    event_sender: broadcast::Sender<Event>,
) {
    // Map of uuid -> (CommandSender, JoinHandle) for per-device tasks
    // CommandSender allows sending messages to running handlers (e.g., to add events device)
    // We no longer track transport here - it's managed within the device handler
    let device_handles = Arc::new(tokio::sync::Mutex::new(HashMap::<
        [u8; 6],
        (mpsc::Sender<DeviceTaskMessage>, tokio::task::JoinHandle<()>),
    >::new()));
    // UUIDs of handlers that were created as direct-USB connections (not BLE-mux primary).
    // These do not need AddControlDevice on each scan pass — their control device is already
    // embedded in the handler. Only UsbMux-primary handlers benefit from AddControlDevice.
    let direct_usb_handler_uuids = Arc::new(tokio::sync::Mutex::new(
        std::collections::HashSet::<[u8; 6]>::new(),
    ));

    // Hub managers: Vec of (DeviceInfo, slot-with-joinhandle)
    let hub_managers = Arc::new(tokio::sync::Mutex::new(Vec::<(
        nusb::DeviceInfo,
        std::sync::Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    )>::new()));
    let mut direct_usb_id_to_uuid = std::collections::HashMap::<String, [u8; 6]>::new();

    loop {
        let devices = match nusb::list_devices().await {
            Ok(iter) => iter.collect::<Vec<_>>(),
            Err(e) => {
                warn!("Failed to list USB devices: {}", e);
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        tracing::info!("USB scan: {} device(s) enumerated", devices.len());
        for dev in &devices {
            tracing::info!(
                "USB scan entry: id={:?} VID={:04x} PID={:04x} serial={:?}",
                dev.id(),
                dev.vendor_id(),
                dev.product_id(),
                dev.serial_number()
            );
            tracing::debug!("USB scan entry full: {:?}", dev);
        }

        // Detect removed hubs and abort their managers (compare by DeviceInfo.id()).
        {
            let mut hm_guard = hub_managers.lock().await;
            let mut to_remove_idx = Vec::<usize>::new();
            for (idx, (stored_info, _slot)) in hm_guard.iter().enumerate() {
                let still_present = devices.iter().any(|d| {
                    d.vendor_id() == 0x1915
                        && d.product_id() == 0x5212
                        && d.id() == stored_info.id()
                });
                if !still_present {
                    to_remove_idx.push(idx);
                }
            }

            for idx in to_remove_idx.into_iter().rev() {
                let (_info, slot_arc) = hm_guard.remove(idx);
                let handle_opt = {
                    let mut slot = slot_arc.lock().await;
                    slot.take()
                };
                if let Some(handle) = handle_opt {
                    tokio::spawn(async move {
                        handle.abort();
                        match tokio::time::timeout(Duration::from_secs(5), handle).await {
                            Ok(r) => tracing::info!("Hub manager task exited: {:?}", r),
                            Err(_) => tracing::info!("Timed out waiting for hub manager to exit"),
                        }
                    });
                }
            }
        }

        // Clean up finished device handlers before checking for new connections.
        // This ensures stale entries don't block reconnecting devices (e.g. after DFU).
        {
            let mut dh = device_handles.lock().await;
            let mut to_remove_uuids = Vec::<[u8; 6]>::new();

            for (uuid, (_cmd_tx, handle)) in dh.iter() {
                if handle.is_finished() {
                    info!("Device {:02x?} task finished, cleaning up", uuid);
                    to_remove_uuids.push(*uuid);
                }
            }

            for uuid in to_remove_uuids {
                if let Some((_cmd_tx, handle)) = dh.remove(&uuid) {
                    handle.abort();
                    info!("Removed finished device handler for {:02x?}", uuid);
                }
                direct_usb_handler_uuids.lock().await.remove(&uuid);
            }
        }

        let mut present_direct_usb_ids = std::collections::HashSet::<String>::new();
        for dev_info in &devices {
            if dev_info.vendor_id() != 0x1915 {
                continue;
            }

            let pid = dev_info.product_id();

            // Skip non-ATS devices
            if !matches!(pid, 0x520f | 0x5210 | 0x5211 | 0x5212) {
                continue;
            }

            // Note: We no longer skip ATS Lites in BLE mode - we enumerate them for DFU/pairing support
            // The device will be connected using connect_usb_any_mode() which works regardless of transport mode

            if pid == 0x5212 {
                // Hub/dongle (0x5212) - check if we're already managing this hub
                let mut hub_managers_guard = hub_managers.lock().await;

                // Find if this hub is in our list
                let existing_idx = hub_managers_guard
                    .iter()
                    .position(|(stored_info, _)| stored_info.id() == dev_info.id());

                if let Some(idx) = existing_idx {
                    // Check if the task is still running
                    let (_, handle_arc) = &hub_managers_guard[idx];
                    let handle_guard = handle_arc.lock().await;
                    let task_finished = handle_guard
                        .as_ref()
                        .map(|h| h.is_finished())
                        .unwrap_or(true);
                    drop(handle_guard);

                    if task_finished {
                        // Task finished, remove from list so we can reconnect
                        info!("Hub manager task finished for {:?}, removing from list to allow reconnect", dev_info.id());
                        hub_managers_guard.remove(idx);
                    } else {
                        // Task still running, skip
                        trace!("Hub already being managed, skipping: {:?}", dev_info.id());
                        drop(hub_managers_guard);
                        continue;
                    }
                }
                drop(hub_managers_guard); // Release lock before connecting

                info!("Attempting to connect to hub/dongle device");
                let hub = match ats_usb::device::MuxDevice::connect_usb(dev_info.clone()).await {
                    Ok(hub) => hub,
                    Err(e) => {
                        warn!("Failed to connect to USB hub device: {}", e);
                        continue;
                    }
                };
                info!("Connected USB hub device: {:?}", dev_info);

                // Subscribe to device list changes
                if let Err(e) = hub.subscribe_device_list().await {
                    tracing::error!("Failed to subscribe to device list: {}", e);
                    continue;
                }
                tracing::info!("Subscribed to device list changes");

                // Hub manager task: handle device snapshots and manage all BLE mux devices
                let hub_clone = hub.clone();
                let device_handles_clone = device_handles.clone();
                let message_channel_clone = message_channel.clone();
                let sc_clone = screen_calibrations.clone();
                let doff_clone = device_offsets.clone();
                let dsd_clone = device_shot_delays.clone();
                let swatch_clone = shot_delay_watch.clone();
                let ev_clone = event_sender.clone();
                let hub_info = dev_info.clone();

                let hub_manager_handle = tokio::spawn(async move {
                    tracing::info!("Hub manager task started for hub {:?}", hub_info);

                    // Track devices managed by this hub for cleanup on exit
                    let mut hub_devices = std::collections::HashSet::<[u8; 6]>::new();

                    // Register dongle in dongle list
                    let dongle_id = hub_info.serial_number().unwrap_or("unknown").to_string();
                    let (dongle_protocol_version, dongle_fw_version) =
                        match hub_clone.read_version().await {
                            Ok(v) => {
                                tracing::info!(
                                    "Dongle protocol {}.{}.{} firmware {}.{}.{}",
                                    v.protocol_semver[0],
                                    v.protocol_semver[1],
                                    v.protocol_semver[2],
                                    v.firmware_semver[0],
                                    v.firmware_semver[1],
                                    v.firmware_semver[2]
                                );
                                (
                                    [
                                        v.protocol_semver[0],
                                        v.protocol_semver[1],
                                        v.protocol_semver[2],
                                    ],
                                    [
                                        v.firmware_semver[0],
                                        v.firmware_semver[1],
                                        v.firmware_semver[2],
                                    ],
                                )
                            }
                            Err(e) => {
                                tracing::warn!("Failed to read dongle version: {}", e);
                                ([0, 0, 0], [0, 0, 0])
                            }
                        };
                    let dongle_control_protocol_version = match hub_clone.read_ctrl_version().await
                    {
                        Ok(v) => {
                            tracing::info!(
                                "Dongle control protocol {}.{}.{}",
                                v.protocol_semver[0],
                                v.protocol_semver[1],
                                v.protocol_semver[2]
                            );
                            [
                                v.protocol_semver[0],
                                v.protocol_semver[1],
                                v.protocol_semver[2],
                            ]
                        }
                        Err(e) => {
                            tracing::warn!("Failed to read dongle control protocol version: {}", e);
                            [0, 0, 0]
                        }
                    };
                    let supports_list_bonds =
                        semver_at_least(dongle_control_protocol_version, [0, 1, 1]);
                    let bonded_devices = if supports_list_bonds {
                        match hub_clone.list_bonds().await {
                            Ok(bonds) => bonds.into_iter().collect(),
                            Err(e) => {
                                tracing::warn!("Failed to list dongle bonds: {}", e);
                                vec![]
                            }
                        }
                    } else {
                        vec![]
                    };
                    let (dongle_cmd_tx, mut dongle_cmd_rx) = mpsc::channel::<DongleTaskMessage>(32);
                    let dongle_info = DongleInfo {
                        id: dongle_id.clone(),
                        protocol_version: dongle_protocol_version,
                        control_protocol_version: dongle_control_protocol_version,
                        firmware_version: dongle_fw_version,
                        connected_devices: vec![],
                        bonded_devices,
                    };
                    let _ = message_channel_clone
                        .send(Message::DongleConnect(dongle_info, dongle_cmd_tx))
                        .await;
                    tracing::info!("Registered dongle '{}' in dongle list", dongle_id);

                    // Request initial device list with retries
                    let initial_devices = {
                        let mut result = None;
                        for attempt in 1..=3 {
                            match hub_clone.request_devices().await {
                                Ok(list) => {
                                    if !list.is_empty() {
                                        tracing::info!(
                                            "Hub manager found {} devices on startup",
                                            list.len()
                                        );
                                    }
                                    result = Some(list);
                                    break;
                                }
                                Err(e) => {
                                    if attempt < 3 {
                                        tracing::debug!(
                                                    "Failed to request devices (attempt {}): {}, retrying...",
                                                    attempt, e
                                                );
                                        tokio::time::sleep(std::time::Duration::from_millis(200))
                                            .await;
                                    } else {
                                        tracing::warn!(
                                            "Failed to request devices after 3 attempts: {}",
                                            e
                                        );
                                    }
                                }
                            }
                        }
                        result
                    };

                    // Create snapshot stream from hub - will receive DevicesSnapshot
                    // messages automatically when devices connect/disconnect
                    tracing::info!("Creating snapshot stream for hub");
                    let snapshot_stream = stream::unfold(hub_clone.clone(), |hub| async move {
                        tracing::debug!("Waiting for device snapshot from hub...");
                        match hub.receive_snapshot().await {
                            Ok(devs) => {
                                tracing::info!(
                                    "receive_snapshot returned Ok with {} devices",
                                    devs.len()
                                );
                                Some((HubEvent::Snapshot(Ok(devs)), hub))
                            }
                            Err(e) => {
                                tracing::error!("receive_snapshot returned Err: {:?}", e);
                                Some((HubEvent::Snapshot(Err(e)), hub))
                            }
                        }
                    });

                    // If we have initial devices, prepend them to the stream
                    let combined_stream = if let Some(devs) = initial_devices {
                        tracing::info!(
                            "Prepending {} initial devices to snapshot stream",
                            devs.len()
                        );
                        stream::once(async move { HubEvent::Snapshot(Ok(devs)) })
                            .chain(snapshot_stream)
                            .boxed()
                    } else {
                        snapshot_stream.boxed()
                    };

                    tokio::pin!(combined_stream);

                    loop {
                        tokio::select! {
                            Some(event) = combined_stream.next() => {
                        match event {
                            HubEvent::Snapshot(snapshot_result) => {
                                match snapshot_result {
                                    Ok(devs) => {
                                        tracing::info!(
                                            "Hub received device snapshot with {} devices: {:02x?}",
                                            devs.len(),
                                            devs
                                        );
                                        use std::collections::HashSet;
                                        let new_set: HashSet<[u8; 6]> = devs.into_iter().collect();
                                        // Compare against hub_devices (BLE addresses) not device_handles (UUIDs)
                                        let existing_set: HashSet<[u8; 6]> = hub_devices.clone();

                                        tracing::info!("Device diff - new: {:02x?}, existing: {:02x?}, added: {:02x?}, removed: {:02x?}",
                                                        new_set, existing_set,
                                                        new_set.difference(&existing_set).copied().collect::<Vec<_>>(),
                                                        existing_set.difference(&new_set).copied().collect::<Vec<_>>());

                                        // Added devices
                                        for addr in new_set.difference(&existing_set) {
                                            let addr_copy = *addr;
                                            tracing::info!(
                                                "DevicesSnapshot: Adding new device {:02x?}",
                                                addr_copy
                                            );

                                            // Connect via VmDevice to properly handle streaming
                                            match ats_usb::device::VmDevice::connect_via_mux(
                                                hub_clone.clone(),
                                                addr_copy,
                                            )
                                            .await
                                            {
                                                Ok(vm_device) => {
                                                    // Read actual UUID from device
                                                    let uuid = match vm_device
                                                        .read_prop(protodongers::PropKind::Uuid)
                                                        .await
                                                    {
                                                        Ok(protodongers::Props::Uuid(
                                                            uuid_bytes,
                                                        )) => uuid_bytes,
                                                        Ok(other) => {
                                                            tracing::warn!(
                                                                "Expected Uuid prop, got {:?}",
                                                                other
                                                            );
                                                            addr_copy
                                                        }
                                                        Err(e) => {
                                                            tracing::warn!(
                                                                            "Failed to read UUID from device: {}",
                                                                            e
                                                                        );
                                                            addr_copy
                                                        }
                                                    };

                                                    tracing::info!(
                                                        "Device {:02x?} has UUID {:02x?}",
                                                        addr_copy,
                                                        uuid
                                                    );

                                                    // Read firmware version via packet interface (works over BLE)
                                                    let version = match vm_device.request(protodongers::PacketData::ReadVersion()).await {
                                                                    Ok(protodongers::PacketData::ReadVersionResponse(v)) => {
                                                                        tracing::info!(
                                                                            "BLE device {:02x?} firmware version: {}.{}.{}",
                                                                            uuid,
                                                                            v.firmware_semver[0],
                                                                            v.firmware_semver[1],
                                                                            v.firmware_semver[2]
                                                                        );
                                                                        [v.firmware_semver[0], v.firmware_semver[1], v.firmware_semver[2]]
                                                                    }
                                                                    Ok(other) => {
                                                                        tracing::warn!("Unexpected response reading version from BLE device {:02x?}: {:?}", uuid, other);
                                                                        [0, 0, 0]
                                                                    }
                                                                    Err(e) => {
                                                                        tracing::warn!("Failed to read version from BLE device {:02x?}: {}", uuid, e);
                                                                        [0, 0, 0]
                                                                    }
                                                                };

                                                    let product_id = match vm_device
                                                        .read_prop(
                                                            protodongers::PropKind::ProductId,
                                                        )
                                                        .await
                                                    {
                                                        Ok(protodongers::Props::ProductId(id)) => {
                                                            id
                                                        }
                                                        _ => 0,
                                                    };

                                                    let device_meta = Device {
                                                                    uuid,
                                                                    transport: odyssey_hub_common::device::Transport::UsbMux,
                                                                    capabilities: odyssey_hub_common::device::DeviceCapabilities::new(
                                                                        odyssey_hub_common::device::DeviceCapabilities::EVENTS
                                                                    ),
                                                                    firmware_version: version,
                                                                    events_transport: odyssey_hub_common::device::EventsTransport::Bluetooth,
                                                                    events_connected: true, // BLE mux has events
                                                                    product_id,
                                                                };

                                                    // Check if device is already connected via USB direct before creating handler
                                                    let dh = device_handles_clone.lock().await;
                                                    tracing::info!("BLE mux device {:02x?} discovered (hub path), checking if already connected. device_handles has {} entries", uuid, dh.len());
                                                    if let Some((
                                                        existing_cmd_tx,
                                                        _existing_handle,
                                                    )) = dh.get(&uuid)
                                                    {
                                                        // Device already exists, send AddEventsDevice to merge BLE events
                                                        tracing::info!("Device {:02x?} found in device_handles, merging by adding BLE mux for events", uuid);
                                                        let result = existing_cmd_tx
                                                            .send(
                                                                DeviceTaskMessage::AddEventsDevice(
                                                                    vm_device,
                                                                ),
                                                            )
                                                            .await;
                                                        tracing::info!("AddEventsDevice message sent for {:02x?}, result: {:?}", uuid, result);

                                                        // Add to hub_devices so it gets cleaned up when hub disconnects
                                                        hub_devices.insert(uuid);
                                                    } else {
                                                        tracing::info!("Device {:02x?} not found in device_handles, creating new BLE mux handler", uuid);
                                                        // New device, create handler and add it
                                                        drop(dh); // Release lock before spawning

                                                        let (ctrl_tx, ctrl_rx) =
                                                            mpsc::channel::<DeviceTaskMessage>(32);

                                                        // BLE mux devices only have events, no control
                                                        let connections = DeviceConnections {
                                                                        control_device: None,
                                                                        events_device: Some(vm_device),
                                                                        transport_mode: protodongers::control::device::TransportMode::Ble,
                                                                    };

                                                        let dh_handle =
                                                            tokio::spawn(device_handler_task(
                                                                connections,
                                                                device_meta.clone(),
                                                                ctrl_rx,
                                                                sc_clone.clone(),
                                                                doff_clone.clone(),
                                                                dsd_clone.clone(),
                                                                swatch_clone.clone(),
                                                                ev_clone.clone(),
                                                                message_channel_clone.clone(),
                                                            ));

                                                        let mut dh =
                                                            device_handles_clone.lock().await;
                                                        dh.insert(
                                                            uuid,
                                                            (ctrl_tx.clone(), dh_handle),
                                                        );
                                                        hub_devices.insert(uuid);

                                                        let channels = DeviceChannels {
                                                            commands: Some(ctrl_tx),
                                                        };
                                                        let _ = message_channel_clone
                                                            .send(Message::Connect(
                                                                device_meta,
                                                                channels,
                                                            ))
                                                            .await;
                                                    }
                                                }
                                                Err(e) => {
                                                    tracing::warn!(
                                                                    "Failed to connect to hub device {:02x?} during snapshot: {}",
                                                                    addr_copy, e
                                                                );
                                                }
                                            }
                                        }

                                        // Removed devices - only remove devices that were managed by THIS hub
                                        let to_remove: Vec<[u8; 6]> = hub_devices
                                            .iter()
                                            .cloned()
                                            .filter(|a| !new_set.contains(a))
                                            .collect();
                                        for addr in to_remove {
                                            let uuid = addr;
                                            tracing::info!(
                                                        "DevicesSnapshot: BLE device {:02x?} disconnected from hub",
                                                        uuid
                                                    );

                                            // Remove from hub_devices tracking
                                            hub_devices.remove(&uuid);

                                            // Check if this device has a handler - if so, send RemoveEventsDevice
                                            // instead of destroying it (it might still have USB control)
                                            let dh = device_handles_clone.lock().await;
                                            if let Some((cmd_tx, _handle)) = dh.get(&uuid) {
                                                // Device handler exists - send RemoveEventsDevice to let it decide
                                                // whether to continue (if it has USB control) or exit
                                                tracing::info!(
                                                            "Sending RemoveEventsDevice to {:02x?} (may still have USB control)",
                                                            uuid
                                                        );
                                                let _ = cmd_tx
                                                    .send(DeviceTaskMessage::RemoveEventsDevice)
                                                    .await;
                                            }
                                            // Don't abort the handler or send Disconnect here -
                                            // the handler will do that itself if it has no remaining connections
                                        }

                                        // Notify dongle list subscribers of connected device changes
                                        let bonded_devices = if supports_list_bonds {
                                            match hub_clone.list_bonds().await {
                                                Ok(bonds) => bonds.into_iter().collect(),
                                                Err(e) => {
                                                    tracing::warn!(
                                                        "Failed to refresh dongle bonds: {}",
                                                        e
                                                    );
                                                    vec![]
                                                }
                                            }
                                        } else {
                                            vec![]
                                        };
                                        let updated_info = DongleInfo {
                                            id: dongle_id.clone(),
                                            protocol_version: dongle_protocol_version,
                                            control_protocol_version: dongle_control_protocol_version,
                                            firmware_version: dongle_fw_version,
                                            connected_devices: hub_devices.iter().copied().collect(),
                                            bonded_devices,
                                        };
                                        let _ = message_channel_clone
                                            .send(Message::DongleUpdate(updated_info))
                                            .await;
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                                "Hub manager receive error - hub likely disconnected: {:?}",
                                                e
                                            );
                                        // Exit immediately on channel closed or other errors
                                        break;
                                    }
                                }
                            }
                        }
                            } // close combined_stream arm

                            Some(msg) = dongle_cmd_rx.recv() => {
                                match msg {
                                    DongleTaskMessage::StartPairing { timeout_ms, reply } => {
                                        let r = hub_clone.start_pairing(timeout_ms).await.map_err(|e| e.to_string());
                                        if r.is_ok() {
                                            let hub_wait = hub_clone.clone();
                                            let ev_tx = ev_clone.clone();
                                            let did = dongle_id.clone();
                                            tokio::spawn(async move {
                                                let (success, paired_address, error) = match hub_wait.wait_pairing_event().await {
                                                    Ok(ats_usb::device::PairingEvent::Result(addr)) => (true, addr, String::new()),
                                                    Ok(ats_usb::device::PairingEvent::Timeout) => (false, [0; 6], "Pairing timed out".into()),
                                                    Ok(ats_usb::device::PairingEvent::Cancelled) => (false, [0; 6], "Pairing cancelled".into()),
                                                    Err(e) => (false, [0; 6], e.to_string()),
                                                };
                                                let _ = ev_tx.send(Event::DonglePairingResult {
                                                    dongle_id: did,
                                                    success,
                                                    paired_address,
                                                    error,
                                                });
                                            });
                                        }
                                        let _ = reply.send(r);
                                    }
                                    DongleTaskMessage::CancelPairing { reply } => {
                                        let r = hub_clone.cancel_pairing().await.map_err(|e| e.to_string());
                                        let _ = reply.send(r);
                                    }
                                    DongleTaskMessage::ClearBonds { reply } => {
                                        let r = hub_clone.clear_bonds().await.map_err(|e| e.to_string());
                                        if r.is_ok() {
                                            let _ = message_channel_clone
                                                .send(Message::DongleUpdate(DongleInfo {
                                                    id: dongle_id.clone(),
                                                    protocol_version: dongle_protocol_version,
                                                    control_protocol_version:
                                                        dongle_control_protocol_version,
                                                    firmware_version: dongle_fw_version,
                                                    connected_devices: hub_devices
                                                        .iter()
                                                        .copied()
                                                        .collect(),
                                                    bonded_devices: vec![],
                                                }))
                                                .await;
                                        }
                                        let _ = reply.send(r);
                                    }
                                }
                            }

                            else => break,
                        } // close select!
                    } // close loop

                    // Unregister dongle
                    let _ = message_channel_clone
                        .send(Message::DongleDisconnect(dongle_id.clone()))
                        .await;
                    tracing::info!("Unregistered dongle '{}' from dongle list", dongle_id);

                    // Cleanup: remove BLE events from devices managed by this hub
                    tracing::info!(
                        "Hub manager task ending for hub {:?}, cleaning up {} devices",
                        hub_info,
                        hub_devices.len()
                    );
                    for uuid in hub_devices {
                        let dh = device_handles_clone.lock().await;
                        if let Some((cmd_tx, _handle)) = dh.get(&uuid) {
                            // Send RemoveEventsDevice to remove BLE mux events
                            // The handler will decide if it should exit (no control) or continue (has USB control)
                            tracing::info!("Sending RemoveEventsDevice for {:02x?}", uuid);
                            let _ = cmd_tx.send(DeviceTaskMessage::RemoveEventsDevice).await;
                        } else {
                            tracing::warn!(
                                "Device {:02x?} not found in device_handles during cleanup",
                                uuid
                            );
                        }
                    }
                    tracing::info!("Hub manager task ended for hub {:?}", hub_info);
                });

                // Store hub manager handle for tracking (runs indefinitely until hub is disconnected)
                {
                    let mut hm = hub_managers.lock().await;
                    // Store with a dummy slot - we track the handle itself
                    let hub_slot =
                        std::sync::Arc::new(tokio::sync::Mutex::new(Some(hub_manager_handle)));
                    hm.push((dev_info.clone(), hub_slot));
                }
            } else {
                // Direct USB device (VmDevice) - 0x520F, 0x5210, or 0x5211
                // First check if we should even connect to this device
                let direct_usb_id = format!("{:?}", dev_info.id());
                present_direct_usb_ids.insert(direct_usb_id.clone());
                debug!(
                    "Found direct USB device PID: 0x{:04x}, checking if already connected",
                    pid
                );

                // Connect and read UUID/version/transport mode first
                // We'll keep this connection if we decide to use it
                let (uuid, version, transport_mode, vm_device_temp) = {
                    match VmDevice::connect_usb_any_mode(dev_info.clone()).await {
                        Ok(vm_device) => {
                            // Read UUID - use control plane for devices that support it (ATS Lite/Lite1), packets for others
                            let uuid = if pid == 0x5210 || pid == 0x5211 {
                                // ATS Lite/Lite1 - use control plane (works in any transport mode)
                                // Retry up to 3 times with delays
                                let mut uuid_result = None;
                                for attempt in 1..=3 {
                                    match vm_device.read_uuid().await {
                                        Ok(uuid) => {
                                            uuid_result = Some(uuid);
                                            break;
                                        }
                                        Err(e) => {
                                            if attempt < 3 {
                                                debug!(
                                                    "ATS Lite: Failed to read UUID via control (attempt {}): {}, retrying...",
                                                    attempt, e
                                                );
                                                sleep(Duration::from_millis(500)).await;
                                            } else {
                                                warn!(
                                                    "ATS Lite: Failed to read UUID via control after 3 attempts: {}, skipping",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                }
                                match uuid_result {
                                    Some(uuid) => uuid,
                                    None => continue,
                                }
                            } else {
                                // Other devices - use packet-based reading with retries
                                let mut uuid_result = None;
                                for attempt in 1..=3 {
                                    match vm_device.read_prop(protodongers::PropKind::Uuid).await {
                                        Ok(protodongers::Props::Uuid(bytes)) => {
                                            uuid_result = Some(bytes);
                                            break;
                                        }
                                        Ok(other) => {
                                            warn!(
                                                "Expected Uuid prop, got {:?}, skipping device",
                                                other
                                            );
                                            break; // Don't retry on wrong response type
                                        }
                                        Err(e) => {
                                            if attempt < 3 {
                                                debug!(
                                                    "Failed to read UUID (attempt {}): {}, retrying...",
                                                    attempt, e
                                                );
                                                sleep(Duration::from_millis(500)).await;
                                            } else {
                                                warn!("Failed to read UUID after 3 attempts: {}, skipping device", e);
                                            }
                                        }
                                    }
                                }
                                match uuid_result {
                                    Some(uuid) => uuid,
                                    None => continue,
                                }
                            };

                            // Read firmware version via control plane (works in any transport mode)
                            let version = match vm_device.read_version().await {
                                Ok(v) => {
                                    info!(
                                        "Device {:02x?} firmware version: {}.{}.{}",
                                        uuid,
                                        v.firmware_semver[0],
                                        v.firmware_semver[1],
                                        v.firmware_semver[2]
                                    );
                                    Some(v)
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to read version from device {:02x?}: {}",
                                        uuid, e
                                    );
                                    None
                                }
                            };

                            // Check transport mode
                            let transport_mode = match vm_device.get_transport_mode().await {
                                Ok(mode) => {
                                    info!("Device {:02x?} is in {:?} mode", uuid, mode);
                                    mode
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to read transport mode from device {:02x?}: {}",
                                        uuid, e
                                    );
                                    // Assume USB mode if we can't read it
                                    protodongers::control::device::TransportMode::Usb
                                }
                            };

                            (uuid, version, transport_mode, vm_device)
                        }
                        Err(e) => {
                            debug!("Failed to connect to USB device: {}", e);
                            continue;
                        }
                    }
                    // Keep vm_device alive - we'll either use it or drop it based on should_connect
                };

                // Cache mapping from opaque nusb id() to UUID once we've successfully probed.
                direct_usb_id_to_uuid.insert(direct_usb_id, uuid);

                // Now check if device is already connected BEFORE creating a new connection
                let should_connect = {
                    let dh = device_handles.lock().await;
                    info!(
                        "Checking if device {:02x?} is already connected (current handles: {})",
                        uuid,
                        dh.len()
                    );

                    // Check if device is already connected
                    if let Some((existing_cmd_tx, _existing_handle)) = dh.get(&uuid) {
                        let is_direct = direct_usb_handler_uuids.lock().await.contains(&uuid);
                        if is_direct {
                            // Already a fully-established direct-USB handler — no merge needed.
                            // Sending AddControlDevice every scan cycle would re-read transport
                            // mode on stale VmDevice instances and corrupt the handler's state.
                            info!(
                                "Device {:02x?} already has direct-USB handler, skipping AddControlDevice",
                                uuid
                            );
                        } else {
                            // BLE-mux primary handler — merge incoming USB control.
                            info!(
                                "Device {:02x?} has BLE-mux handler, sending AddControlDevice to merge USB control",
                                uuid
                            );
                            let result = existing_cmd_tx
                                .send(DeviceTaskMessage::AddControlDevice(vm_device_temp.clone()))
                                .await;
                            info!(
                                "AddControlDevice message sent for {:02x?}, result: {:?}",
                                uuid, result
                            );
                        }

                        false // Skip creating new handler
                    } else {
                        // Device is not connected
                        info!("Device {:02x?} not in handles, will connect", uuid);
                        true // Connect
                    }
                };

                info!("should_connect for {:02x?}: {}", uuid, should_connect);
                if !should_connect {
                    // Drop the vm_device we created for checking
                    drop(vm_device_temp);
                    continue;
                }

                // Device is not connected or we're replacing BLE mux connection
                // Reuse the connection we already created
                info!(
                    "Connecting new USB device with UUID {:02x?}, PID: 0x{:04x}",
                    uuid, pid
                );

                // Determine capabilities based on transport mode
                // USB direct always has CONTROL capability
                // EVENTS capability depends on transport mode (USB mode has events, BLE mode doesn't)
                let mut capabilities = odyssey_hub_common::device::DeviceCapabilities::new(
                    odyssey_hub_common::device::DeviceCapabilities::CONTROL,
                );
                if transport_mode == protodongers::control::device::TransportMode::Usb {
                    capabilities.insert(odyssey_hub_common::device::DeviceCapabilities::EVENTS);
                }

                let device_meta = Device {
                    uuid,
                    transport: odyssey_hub_common::device::Transport::Usb,
                    capabilities,
                    firmware_version: version
                        .as_ref()
                        .map(|v| {
                            [
                                v.firmware_semver[0],
                                v.firmware_semver[1],
                                v.firmware_semver[2],
                            ]
                        })
                        .unwrap_or([0, 0, 0]),
                    events_transport: if transport_mode
                        == protodongers::control::device::TransportMode::Usb
                    {
                        odyssey_hub_common::device::EventsTransport::Wired
                    } else {
                        odyssey_hub_common::device::EventsTransport::Bluetooth
                    },
                    events_connected: transport_mode
                        == protodongers::control::device::TransportMode::Usb,
                    product_id: pid,
                };

                let (tx, rx) = mpsc::channel(32);

                // USB direct devices: always have control, events only if in USB mode
                let connections =
                    if transport_mode == protodongers::control::device::TransportMode::Usb {
                        // USB mode: same device for both control and events
                        DeviceConnections {
                            control_device: Some(vm_device_temp.clone()),
                            events_device: Some(vm_device_temp),
                            transport_mode,
                        }
                    } else {
                        // BLE mode: control only, no events
                        DeviceConnections {
                            control_device: Some(vm_device_temp),
                            events_device: None,
                            transport_mode,
                        }
                    };

                let handle = tokio::spawn(device_handler_task(
                    connections,
                    device_meta.clone(),
                    rx,
                    screen_calibrations.clone(),
                    device_offsets.clone(),
                    device_shot_delays.clone(),
                    shot_delay_watch.clone(),
                    event_sender.clone(),
                    message_channel.clone(),
                ));

                {
                    let mut dh = device_handles.lock().await;
                    dh.insert(uuid, (tx.clone(), handle));
                    direct_usb_handler_uuids.lock().await.insert(uuid);
                }

                // Always pass the commands channel so server.device_list gets a working sender.
                // The device handler task will process commands (like Zero) once events are available.
                let channels = DeviceChannels { commands: Some(tx) };

                let _ = message_channel
                    .send(Message::Connect(device_meta, channels))
                    .await;
            }
        }

        // Reconcile direct USB disconnects using opaque id() keys.
        // This avoids requiring UUID re-probe success on every scan.
        let missing_direct_usb_ids: Vec<String> = direct_usb_id_to_uuid
            .keys()
            .filter(|id| !present_direct_usb_ids.contains(*id))
            .cloned()
            .collect();
        if !missing_direct_usb_ids.is_empty() {
            let dh = device_handles.lock().await;
            for id in missing_direct_usb_ids {
                if let Some(uuid) = direct_usb_id_to_uuid.remove(&id) {
                    if let Some((cmd_tx, _)) = dh.get(&uuid) {
                        tracing::info!(
                            "Direct USB device missing from scan, sending RemoveControlDevice: id={} uuid={:02x?}",
                            id,
                            uuid
                        );
                        let _ = cmd_tx.send(DeviceTaskMessage::RemoveControlDevice).await;
                    } else {
                        tracing::info!(
                            "Direct USB device missing from scan but no active handler: id={} uuid={:02x?}",
                            id,
                            uuid
                        );
                    }
                }
            }
        }

        sleep(Duration::from_secs(2)).await;
    }
}

/// Start all sensor streams for a given events device.
/// Returns a SelectAll of boxed streams.
async fn start_sensor_streams(
    events_device: &VmDevice,
    uuid: [u8; 6],
) -> SelectAll<BoxStream<'static, DeviceStreamEvent>> {
    let mut sensor_streams: SelectAll<BoxStream<'static, DeviceStreamEvent>> = SelectAll::new();
    match events_device.clone().stream_combined_markers().await {
        Ok(stream) => {
            info!("Combined markers stream started for {:02x?}", uuid);
            sensor_streams.push(stream.map(DeviceStreamEvent::CombinedMarkers).boxed());
        }
        Err(e) => warn!("Failed to start combined markers stream for {:02x?}: {}", uuid, e),
    }
    match events_device.clone().stream_poc_markers().await {
        Ok(stream) => {
            info!("PoC markers stream started for {:02x?}", uuid);
            sensor_streams.push(stream.map(DeviceStreamEvent::PocMarkers).boxed());
        }
        Err(e) => warn!("Failed to start PoC markers stream for {:02x?}: {}", uuid, e),
    }
    match events_device.clone().stream_accel().await {
        Ok(stream) => {
            info!("Accelerometer stream started for {:02x?}", uuid);
            sensor_streams.push(stream.map(DeviceStreamEvent::Accel).boxed());
        }
        Err(e) => warn!("Failed to start accelerometer stream for {:02x?}: {}", uuid, e),
    }
    match events_device.clone().stream_impact().await {
        Ok(stream) => {
            info!("Impact stream started for {:02x?}", uuid);
            sensor_streams.push(stream.map(DeviceStreamEvent::Impact).boxed());
        }
        Err(e) => warn!("Failed to start impact stream for {:02x?}: {}", uuid, e),
    }
    sensor_streams
}

/// Device connections - can have separate VmDevice instances for control vs events
struct DeviceConnections {
    /// VmDevice for control operations (USB direct connection)
    control_device: Option<VmDevice>,
    /// VmDevice for event streams (USB direct or BLE mux)
    events_device: Option<VmDevice>,
    /// Transport mode of the primary device
    transport_mode: protodongers::control::device::TransportMode,
}

/// Per-device handler: receives packets from the link, processes control messages,
/// watches for shot-delay changes, and emits events.
async fn device_handler_task(
    mut connections: DeviceConnections,
    mut device: Device,
    control_rx: mpsc::Receiver<DeviceTaskMessage>,
    screen_calibrations: Arc<
        ArcSwap<
            ArrayVec<(u8, ScreenCalibration<f32>), { (ats_common::MAX_SCREEN_ID + 1) as usize }>,
        >,
    >,
    device_offsets: Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
    device_shot_delays: Arc<RwLock<HashMap<[u8; 6], u16>>>,
    shot_delay_watch: Arc<RwLock<HashMap<[u8; 6], watch::Sender<u32>>>>,
    event_sender: broadcast::Sender<Event>,
    message_channel: mpsc::Sender<Message>,
) {
    info!(
        "Starting device handler for device UUID {:02x?}",
        device.uuid
    );

    // Ensure shot-delay watch exists and subscribe
    let shot_delay_rx = {
        let mut map = shot_delay_watch.write().await;
        if let Some(tx) = map.get(&device.uuid) {
            tx.subscribe()
        } else {
            let cur = device_shot_delays
                .read()
                .await
                .get(&device.uuid)
                .copied()
                .unwrap_or(0) as u32;
            let (tx, rx) = watch::channel(cur);
            let _ = tx.send(cur);
            map.insert(device.uuid, tx);
            rx
        }
    };

    // Track current value locally
    let mut current_shot_delay = *shot_delay_rx.borrow();
    info!(
        "Initial shot delay for {:02x?} = {} ms",
        device.uuid, current_shot_delay
    );
    // Per-device foveated state (local to this handler) and previous IMU timestamp.
    // Keeping these local avoids a global registry and ties filter lifetime to the device task.
    let mut fv_state = FoveatedAimpointState::new();
    let mut prev_accel_timestamp: Option<u32> = None;
    let mut madgwick = ahrs::Madgwick::<f32>::new(1.0 / 100.0, 0.1);
    let mut orientation = Rotation3::identity();

    // Read camera config. Devices with bulk packet support (0x5200, etc.) use the events
    // device. ATS Lite (0x5210) and Lite1 (0x5211) use the control plane (EP0) instead,
    // since their bulk handler is gated on USB transport mode being active.
    let supports_bulk_config = !matches!(device.product_id, 0x5210 | 0x5211);
    let general_settings = if let Some(events_device) = connections.events_device.as_ref().filter(|_| supports_bulk_config) {
        info!(
            "Reading camera config for device {:02x?} from events device (bulk)",
            device.uuid
        );
        match tokio::time::timeout(Duration::from_secs(5), events_device.read_all_config()).await {
            Ok(Ok(cfg)) => {
                info!(
                    "Successfully read camera config for device {:02x?}",
                    device.uuid
                );
                Some(cfg)
            }
            Ok(Err(e)) => {
                warn!(
                    "Failed to read camera config for device {:02x?}: {}",
                    device.uuid, e
                );
                None
            }
            Err(_) => {
                warn!(
                    "read_all_config timed out for device {:02x?}, proceeding with defaults",
                    device.uuid
                );
                None
            }
        }
    } else if !supports_bulk_config {
        if let Some(ctrl_device) = connections.control_device.as_ref() {
            info!(
                "Reading camera config for device {:02x?} via control plane",
                device.uuid
            );
            match tokio::time::timeout(Duration::from_secs(10), ctrl_device.read_all_config_ctrl()).await {
                Ok(Ok(cfg)) => {
                    info!(
                        "Successfully read camera config for device {:02x?} via control plane",
                        device.uuid
                    );
                    Some(cfg)
                }
                Ok(Err(e)) => {
                    warn!(
                        "Failed to read camera config for device {:02x?} via control plane: {}",
                        device.uuid, e
                    );
                    None
                }
                Err(_) => {
                    warn!(
                        "read_all_config_ctrl timed out for device {:02x?}, proceeding with defaults",
                        device.uuid
                    );
                    None
                }
            }
        } else {
            info!(
                "Skipping camera config for device {:02x?} (no control device available)",
                device.uuid
            );
            None
        }
    } else {
        info!(
            "Skipping camera config for device {:02x?} (no events device)",
            device.uuid
        );
        None
    };

    info!("Setting up camera models for device {:02x?}", device.uuid);
    let (mut camera_model_nf, mut camera_model_wf, mut stereo_iso) =
        if let Some(ref cfg) = general_settings {
            (
                cfg.camera_model_nf.clone(),
                cfg.camera_model_wf.clone(),
                cfg.stereo_iso.clone(),
            )
        } else {
            // Device in BLE mode - use dummy camera models
            // These will be updated when AddEventsDevice is called
            info!(
                "Device {:02x?} starting in BLE mode with dummy camera models",
                device.uuid
            );
            (
                opencv_ros_camera::RosOpenCvIntrinsics::from_params(1.0, 0.0, 1.0, 0.0, 0.0),
                opencv_ros_camera::RosOpenCvIntrinsics::from_params(1.0, 0.0, 1.0, 0.0, 0.0),
                nalgebra::Isometry3::identity(),
            )
        };

    info!("Starting sensor streams for device {:02x?}", device.uuid);
    // Start sensor streams via VmDevice helpers so each stream reserves a slot
    // Only start streams if we have an events device (USB mode or BLE mux)
    let mut sensor_streams: SelectAll<BoxStream<'static, DeviceStreamEvent>> = SelectAll::new();

    if let Some(events_device) = connections.events_device.as_ref() {
        sensor_streams = start_sensor_streams(events_device, device.uuid).await;
    } else {
        info!(
            "Skipping sensor streams for device {:02x?} (no events device)",
            device.uuid
        );
    }

    // If no sensor streams started (e.g., device in BLE mode),
    // we still want to keep the device connected for control (DFU, etc.)
    // Add a health check stream that periodically checks if USB is still connected
    let mut has_sensor_streams = !sensor_streams.is_empty();
    info!(
        "Device {:02x?} has_sensor_streams: {}",
        device.uuid, has_sensor_streams
    );

    // Keep sensor_streams mutable so we can add streams later (e.g., when BLE mux is added)
    let mut shot_delay_stream = tokio_stream::wrappers::WatchStream::new(shot_delay_rx).fuse();
    let mut control_stream = tokio_stream::wrappers::ReceiverStream::new(control_rx).fuse();

    let pairing_in_progress = Arc::new(std::sync::atomic::AtomicBool::new(false));

    info!(
        "Device handler task entering main loop for {:02x?}, has_sensor_streams={}",
        device.uuid, has_sensor_streams
    );

    loop {
        tokio::select! {
            biased;
            maybe_msg = control_stream.next() => {
                let msg_name = match &maybe_msg {
                    Some(DeviceTaskMessage::ResetZero) => "ResetZero",
                    Some(DeviceTaskMessage::SaveZero) => "SaveZero",
                    Some(DeviceTaskMessage::WriteVendor(..)) => "WriteVendor",
                    Some(DeviceTaskMessage::SetShotDelay(..)) => "SetShotDelay",
                    Some(DeviceTaskMessage::Zero(..)) => "Zero",
                    Some(DeviceTaskMessage::ClearZero) => "ClearZero",
                    Some(DeviceTaskMessage::ReadRegister { .. }) => "ReadRegister",
                    Some(DeviceTaskMessage::WriteRegister { .. }) => "WriteRegister",
                    Some(DeviceTaskMessage::ReadConfig { .. }) => "ReadConfig",
                    Some(DeviceTaskMessage::WriteConfig { .. }) => "WriteConfig",
                    Some(DeviceTaskMessage::FlashSettings { .. }) => "FlashSettings",
                    Some(DeviceTaskMessage::StartPairing { .. }) => "StartPairing",
                    Some(DeviceTaskMessage::CancelPairing { .. }) => "CancelPairing",
                    Some(DeviceTaskMessage::ClearBond { .. }) => "ClearBond",
                    Some(DeviceTaskMessage::SetTransportMode { .. }) => "SetTransportMode",
                    Some(DeviceTaskMessage::AddEventsDevice(..)) => "AddEventsDevice",
                    Some(DeviceTaskMessage::AddControlDevice(..)) => "AddControlDevice",
                    Some(DeviceTaskMessage::RemoveControlDevice) => "RemoveControlDevice",
                    Some(DeviceTaskMessage::RemoveEventsDevice) => "RemoveEventsDevice",
                    None => "None (channel closed)",
                };
                info!("control_stream received '{}' for {:02x?}", msg_name, device.uuid);
                match maybe_msg {
                    Some(DeviceTaskMessage::ResetZero) => {
                        debug!("ResetZero requested for {:02x?}", device.uuid);
                        {
                            let mut guard = device_offsets.lock().await;
                            guard.remove(&device.uuid);
                        }
                        let ev = Event::DeviceEvent(DeviceEvent(
                            device.clone(),
                            DeviceEventKind::ZeroResult(true),
                        ));
                        let _ = event_sender.send(ev);
                    }
                    Some(DeviceTaskMessage::SaveZero) => {
                        debug!("SaveZero requested for {:02x?}", device.uuid);
                        {
                            let guard = device_offsets.lock().await;
                            if let Err(e) =
                                odyssey_hub_common::config::device_offsets_save_async(&*guard).await
                            {
                                tracing::warn!("Failed to save zero offsets: {}", e);
                            }
                        }
                        let ev = Event::DeviceEvent(DeviceEvent(
                            device.clone(),
                            DeviceEventKind::SaveZeroResult(true),
                        ));
                        let _ = event_sender.send(ev);
                    }
                    Some(DeviceTaskMessage::WriteVendor(tag, data)) => {
                        debug!(
                            "WriteVendor tag={} len={} for {:02x?}",
                            tag,
                            data.len(),
                            device.uuid
                        );
                        let clipped_len = std::cmp::min(data.len(), 98);
                        if let Some(events_dev) = connections.events_device.as_ref() {
                            if let Err(e) = events_dev
                                .write_vendor(tag, &data[..clipped_len])
                                .await
                            {
                                warn!("Failed to send vendor packet: {}", e);
                            }
                        } else {
                            warn!("Cannot write vendor packet: no events device available");
                        }
                    }
                    Some(DeviceTaskMessage::SetShotDelay(ms)) => {
                        debug!(
                            "SetShotDelay requested for {:02x?} = {} ms",
                            device.uuid, ms
                        );
                        {
                            device_shot_delays.write().await.insert(device.uuid, ms);
                            let mut g = shot_delay_watch.write().await;
                            if let Some(tx) = g.get(&device.uuid) {
                                let _ = tx.send(ms as u32);
                            } else {
                                let (tx, _rx) = watch::channel(ms as u32);
                                let _ = tx.send(ms as u32);
                                g.insert(device.uuid, tx);
                            }
                        }
                    }
                    Some(DeviceTaskMessage::Zero(trans, point, tracking_event)) => {
                        info!(
                            "Zero received for {:02x?}, translation: {:?}, target: {:?}, has_tracking_event: {}",
                            device.uuid, trans.vector, point, tracking_event.is_some()
                        );
                        let quat = if let Some(te) = tracking_event {
                            let sc = screen_calibrations.load();
                            calculate_zero_offset_from_tracking_event(trans, point, &sc, &te)
                        } else {
                            let sc = screen_calibrations.load();
                            ats_helpers::calculate_zero_offset_quat(trans, point, &sc, &fv_state)
                        };
                        if let Some(quat) = quat {
                            let iso = Isometry3::from_parts(trans, quat);
                            info!("Zero offset computed for {:02x?}: {:?}", device.uuid, iso);
                            {
                                let mut guard = device_offsets.lock().await;
                                guard.insert(device.uuid, iso);
                            }
                            let ev = Event::DeviceEvent(DeviceEvent(
                                device.clone(),
                                DeviceEventKind::ZeroResult(true),
                            ));
                            let _ = event_sender.send(ev);
                        } else {
                            warn!("Zero offset computation failed for {:02x?} (no valid pose)", device.uuid);
                            let ev = Event::DeviceEvent(DeviceEvent(
                                device.clone(),
                                DeviceEventKind::ZeroResult(false),
                            ));
                            let _ = event_sender.send(ev);
                        }
                    }
                    Some(DeviceTaskMessage::ClearZero) => {
                        debug!(
                            "ClearZero - removing stored device offset for {:02x?}",
                            device.uuid
                        );
                        {
                            let mut guard = device_offsets.lock().await;
                            guard.remove(&device.uuid);
                        }
                        let ev = Event::DeviceEvent(DeviceEvent(
                            device.clone(),
                            DeviceEventKind::SaveZeroResult(true),
                        ));
                        let _ = event_sender.send(ev);
                    }
                    Some(DeviceTaskMessage::ReadRegister { port, bank, address, reply }) => {
                        let port = if port == 0 { protodongers::Port::Nf } else { protodongers::Port::Wf };
                        if !device.events_connected {
                            let _ = reply.send(Err("events transport not connected".into()));
                        } else if let Some(dev) = connections.events_device.as_ref().cloned() {
                            // Pause sensor streams to avoid PAG mutex contention on the firmware.
                            // obj_loop holds the PAG mutex during get_objects(); if a ReadRegister
                            // arrives while that lock is held, recv_loop blocks for the full
                            // get_objects() duration. We send DisableAll first (an acknowledged
                            // round-trip) so obj_loop stops before ReadRegister reaches the firmware.
                            let had_streams = has_sensor_streams;
                            if had_streams {
                                sensor_streams = SelectAll::new();
                                has_sensor_streams = false;
                                // clear_all_streams() sends DisableAll and waits for Ack, ensuring
                                // the firmware has processed DisableAll before we send ReadRegister.
                                if let Err(e) = dev.clear_all_streams().await {
                                    warn!("clear_all_streams failed before ReadRegister for {:02x?}: {}", device.uuid, e);
                                }
                            }
                            let result = match tokio::time::timeout(Duration::from_secs(3), dev.read_register(port, bank, address)).await {
                                Ok(r) => r.map_err(|e| e.to_string()),
                                Err(_) => Err("read_register timed out".into()),
                            };
                            let _ = reply.send(result);
                            // Restart streams now that the register read is done.
                            if had_streams {
                                sensor_streams = start_sensor_streams(&dev, device.uuid).await;
                                has_sensor_streams = !sensor_streams.is_empty();
                            }
                        } else {
                            let _ = reply.send(Err("events device not available".into()));
                        }
                    }
                    Some(DeviceTaskMessage::WriteRegister { port, bank, address, data, reply }) => {
                        let port = if port == 0 { protodongers::Port::Nf } else { protodongers::Port::Wf };
                        if !device.events_connected {
                            let _ = reply.send(Err("events transport not connected".into()));
                        } else if let Some(dev) = connections.events_device.as_ref().cloned() {
                            // Pause streams while writing registers for the same reason as ReadRegister.
                            let had_streams = has_sensor_streams;
                            if had_streams {
                                sensor_streams = SelectAll::new();
                                has_sensor_streams = false;
                                if let Err(e) = dev.clear_all_streams().await {
                                    warn!("clear_all_streams failed before WriteRegister for {:02x?}: {}", device.uuid, e);
                                }
                            }
                            let result = match tokio::time::timeout(Duration::from_secs(3), dev.write_register(port, bank, address, data)).await {
                                Ok(r) => r.map_err(|e| e.to_string()),
                                Err(_) => Err("write_register timed out".into()),
                            };
                            let _ = reply.send(result);
                            if had_streams {
                                sensor_streams = start_sensor_streams(&dev, device.uuid).await;
                                has_sensor_streams = !sensor_streams.is_empty();
                            }
                        } else {
                            let _ = reply.send(Err("events device not available".into()));
                        }
                    }
                    Some(DeviceTaskMessage::ReadConfig { kind, reply }) => {
                        let config_kind = match kind {
                            0 => Some(ConfigKind::ImpactThreshold),
                            1 => Some(ConfigKind::SuppressMs),
                            _ => None,
                        };
                        if let Some(ck) = config_kind {
                            if let Some(dev) = connections.events_device.as_ref().filter(|_| supports_bulk_config) {
                                let result = match tokio::time::timeout(Duration::from_secs(3), dev.read_config(ck)).await {
                                    Ok(Ok(gc)) => Ok(match gc {
                                        GeneralConfig::ImpactThreshold(v) => v,
                                        GeneralConfig::SuppressMs(v) => v,
                                        _ => 0,
                                    }),
                                    Ok(Err(e)) => Err(e.to_string()),
                                    Err(_) => Err("read_config timed out".into()),
                                };
                                let _ = reply.send(result);
                            } else if let Some(dev) = connections.control_device.as_ref() {
                                let result = match tokio::time::timeout(Duration::from_secs(3), dev.read_config_ctrl(ck)).await {
                                    Ok(Ok(gc)) => Ok(match gc {
                                        GeneralConfig::ImpactThreshold(v) => v,
                                        GeneralConfig::SuppressMs(v) => v,
                                        _ => 0,
                                    }),
                                    Ok(Err(e)) => Err(e.to_string()),
                                    Err(_) => Err("read_config_ctrl timed out".into()),
                                };
                                let _ = reply.send(result);
                            } else {
                                let _ = reply.send(Err("no device available for read_config".into()));
                            }
                        } else {
                            let _ = reply.send(Err(format!("unknown config kind: {kind}")));
                        }
                    }
                    Some(DeviceTaskMessage::WriteConfig { kind, value, reply }) => {
                        let config = match kind {
                            0 => Some(GeneralConfig::ImpactThreshold(value)),
                            1 => Some(GeneralConfig::SuppressMs(value)),
                            _ => None,
                        };
                        if let Some(gc) = config {
                            if let Some(dev) = connections.events_device.as_ref().filter(|_| supports_bulk_config) {
                                let result = match tokio::time::timeout(Duration::from_secs(3), dev.write_config(gc)).await {
                                    Ok(r) => r.map_err(|e| e.to_string()),
                                    Err(_) => Err("write_config timed out".into()),
                                };
                                let _ = reply.send(result);
                            } else if let Some(dev) = connections.control_device.as_ref() {
                                let result = match tokio::time::timeout(Duration::from_secs(3), dev.write_config_ctrl(gc)).await {
                                    Ok(r) => r.map_err(|e| e.to_string()),
                                    Err(_) => Err("write_config_ctrl timed out".into()),
                                };
                                let _ = reply.send(result);
                            } else {
                                let _ = reply.send(Err("no device available for write_config".into()));
                            }
                        } else {
                            let _ = reply.send(Err(format!("unknown config kind: {kind}")));
                        }
                    }
                    Some(DeviceTaskMessage::FlashSettings { reply }) => {
                        if let Some(dev) = connections.events_device.as_ref().filter(|_| supports_bulk_config) {
                            let result = match tokio::time::timeout(Duration::from_secs(3), dev.flash_settings()).await {
                                Ok(r) => r.map_err(|e| e.to_string()),
                                Err(_) => Err("flash_settings timed out".into()),
                            };
                            let _ = reply.send(result);
                        } else if let Some(dev) = connections.control_device.as_ref() {
                            let result = match tokio::time::timeout(Duration::from_secs(5), dev.flash_settings_ctrl()).await {
                                Ok(r) => r.map_err(|e| e.to_string()),
                                Err(_) => Err("flash_settings_ctrl timed out".into()),
                            };
                            let _ = reply.send(result);
                        } else {
                            let _ = reply.send(Err("no device available for flash_settings".into()));
                        }
                    }
                    Some(DeviceTaskMessage::StartPairing { timeout_ms, reply }) => {
                        if let Some(dev) = connections.control_device.as_ref() {
                            info!("StartPairing: sending start_pairing({}ms) to device {:02x?}", timeout_ms, device.uuid);
                            let r = match tokio::time::timeout(Duration::from_secs(5), dev.start_pairing(timeout_ms)).await {
                                Ok(r) => r.map_err(|e| e.to_string()),
                                Err(_) => Err("start_pairing timed out".into()),
                            };
                            info!("StartPairing result for {:02x?}: {:?}", device.uuid, r);
                            if r.is_ok() {
                                pairing_in_progress.store(true, std::sync::atomic::Ordering::Relaxed);
                                let pairing_flag = pairing_in_progress.clone();
                                let dev_clone = dev.clone();
                                let ev_tx = event_sender.clone();
                                let dev_meta = device.clone();
                                tokio::spawn(async move {
                                    let (success, paired_address, error) = match dev_clone.wait_pairing_event().await {
                                        Ok(addr) => (true, addr, String::new()),
                                        Err(e) => (false, [0; 6], e.to_string()),
                                    };
                                    pairing_flag.store(false, std::sync::atomic::Ordering::Relaxed);
                                    let event = odyssey_hub_common::events::Event::DeviceEvent(
                                        odyssey_hub_common::events::DeviceEvent(
                                            dev_meta,
                                            odyssey_hub_common::events::DeviceEventKind::PairingResult {
                                                success,
                                                paired_address,
                                                error,
                                            },
                                        ),
                                    );
                                    let _ = ev_tx.send(event);
                                });
                            }
                            let _ = reply.send(r);
                        } else {
                            let _ = reply.send(Err("no control device available".into()));
                        }
                    }
                    Some(DeviceTaskMessage::CancelPairing { reply }) => {
                        if let Some(dev) = connections.control_device.as_ref() {
                            info!("CancelPairing: sending cancel to device {:02x?}", device.uuid);
                            let r = match tokio::time::timeout(Duration::from_secs(3), dev.cancel_pairing()).await {
                                Ok(r) => r.map_err(|e| e.to_string()),
                                Err(_) => Err("cancel_pairing timed out".into()),
                            };
                            info!("CancelPairing result for {:02x?}: {:?}", device.uuid, r);
                            // Note: pairing_in_progress flag will be cleared by the wait_pairing_event
                            // task when it receives the Cancelled result
                            let _ = reply.send(r);
                        } else {
                            warn!("CancelPairing: no control device for {:02x?}", device.uuid);
                            let _ = reply.send(Err("no control device available".into()));
                        }
                    }
                    Some(DeviceTaskMessage::ClearBond { reply }) => {
                        if let Some(dev) = connections.control_device.as_ref() {
                            info!("ClearBond: sending clear to device {:02x?}", device.uuid);
                            let r = match tokio::time::timeout(Duration::from_secs(3), dev.clear_bond()).await {
                                Ok(r) => r.map_err(|e| e.to_string()),
                                Err(_) => Err("clear_bond timed out".into()),
                            };
                            info!("ClearBond result for {:02x?}: {:?}", device.uuid, r);
                            let _ = reply.send(r);
                        } else {
                            warn!("ClearBond: no control device for {:02x?}", device.uuid);
                            let _ = reply.send(Err("no control device available".into()));
                        }
                    }
                    Some(DeviceTaskMessage::SetTransportMode { usb_mode, reply }) => {
                        if let Some(dev) = connections.control_device.clone() {
                            info!("SetTransportMode: usb_mode={} for device {:02x?}", usb_mode, device.uuid);
                            let r = match tokio::time::timeout(Duration::from_secs(3), dev.set_transport_mode(usb_mode)).await {
                                Ok(Ok(mode)) => Ok(matches!(mode, protodongers::control::device::TransportMode::Usb)),
                                Ok(Err(e)) => Err(e.to_string()),
                                Err(_) => Err("set_transport_mode timed out".into()),
                            };
                            info!("SetTransportMode result for {:02x?}: {:?}", device.uuid, r);
                            if r.is_ok() {
                                // Persist the new transport mode to flash before rebooting,
                                // otherwise the device comes back on the old mode after reboot.
                                info!("Flashing settings for {:02x?} to persist transport mode", device.uuid);
                                match tokio::time::timeout(Duration::from_secs(5), dev.flash_settings_ctrl()).await {
                                    Ok(Ok(())) => info!("Flash settings ok for {:02x?}", device.uuid),
                                    Ok(Err(e)) => warn!("Flash settings failed for {:02x?}: {}", device.uuid, e),
                                    Err(_) => warn!("Flash settings timed out for {:02x?}", device.uuid),
                                }
                            }
                            // Reply to the RPC caller before rebooting.
                            let _ = reply.send(r);
                            // Reboot the device so it comes back clean in the new mode.
                            // We don't wait for the full ack timeout — just fire and break
                            // immediately so the device disappears from the UI before the
                            // reboot completes. This prevents stale RPC calls (e.g.
                            // ReadRegister) from hitting a dead device while it is resetting.
                            info!("Rebooting device {:02x?} to apply transport mode change", device.uuid);
                            let _ = tokio::time::timeout(Duration::from_millis(500), dev.reboot()).await;
                            break;
                        } else {
                            warn!("SetTransportMode: no control device for {:02x?}", device.uuid);
                            let _ = reply.send(Err("no control device available".into()));
                        }
                    }
                    Some(DeviceTaskMessage::AddEventsDevice(events_vm_device)) => {
                        info!(
                            "AddEventsDevice - adding BLE mux events device for {:02x?}",
                            device.uuid
                        );

                        // Store the events device
                        connections.events_device = Some(events_vm_device.clone());

                        // Start all sensor streams with the new events device
                        sensor_streams = start_sensor_streams(&events_vm_device, device.uuid).await;

                        // Update has_sensor_streams flag
                        has_sensor_streams = !sensor_streams.is_empty();

                        // Update device config from BLE mux device
                        // This is critical when a device starts in BLE mode (no config)
                        // and later gets BLE mux events added
                        info!("Reading device config from BLE mux device for {:02x?}", device.uuid);
                        match tokio::time::timeout(Duration::from_secs(5), events_vm_device.read_all_config()).await {
                            Ok(Ok(cfg)) => {
                                info!("Updating device config for {:02x?} from BLE mux", device.uuid);
                                // Update camera models from the new config
                                camera_model_nf = cfg.camera_model_nf.clone();
                                camera_model_wf = cfg.camera_model_wf.clone();
                                stereo_iso = cfg.stereo_iso.clone();
                            }
                            Ok(Err(e)) => {
                                warn!("Failed to read device config from BLE mux {:02x?}: {}", device.uuid, e);
                            }
                            Err(_) => {
                                warn!("read_all_config timed out for BLE mux {:02x?}", device.uuid);
                            }
                        }

                        // Update device capabilities to include EVENTS
                        device.capabilities.insert(DeviceCapabilities::EVENTS);

                        // Mark events as connected since BLE mux streams are now active
                        device.events_connected = true;

                        // Notify clients about the capability change
                        let ev = Event::DeviceEvent(DeviceEvent(
                            device.clone(),
                            DeviceEventKind::CapabilitiesChanged,
                        ));
                        let _ = event_sender.send(ev);

                        // Update the device in the device list
                        let _ = message_channel.send(Message::UpdateDevice(device.clone())).await;

                        info!("BLE mux events device added successfully for {:02x?}, has_sensor_streams: {}",
                              device.uuid, has_sensor_streams);
                    }
                    Some(DeviceTaskMessage::AddControlDevice(control_vm_device)) => {
                        info!(
                            "AddControlDevice - adding USB control device for {:02x?}",
                            device.uuid
                        );

                        // Always refresh firmware version from USB control when it is (re)attached.
                        // This is critical after DFU: device handler may still hold old version.
                        match tokio::time::timeout(Duration::from_secs(3), control_vm_device.read_version()).await {
                            Ok(Ok(v)) => {
                                let new_fw = [v.firmware_semver[0], v.firmware_semver[1], v.firmware_semver[2]];
                                if device.firmware_version != new_fw {
                                    info!(
                                        "Updating firmware version for {:02x?}: {}.{}.{} -> {}.{}.{}",
                                        device.uuid,
                                        device.firmware_version[0],
                                        device.firmware_version[1],
                                        device.firmware_version[2],
                                        new_fw[0],
                                        new_fw[1],
                                        new_fw[2]
                                    );
                                } else {
                                    info!(
                                        "Read firmware version from USB control for {:02x?}: {}.{}.{}",
                                        device.uuid, new_fw[0], new_fw[1], new_fw[2]
                                    );
                                }
                                device.firmware_version = new_fw;
                            }
                            Ok(Err(e)) => {
                                warn!(
                                    "Failed to read firmware version from USB control for {:02x?}: {}",
                                    device.uuid, e
                                );
                            }
                            Err(_) => {
                                warn!("read_version timed out for USB control {:02x?}", device.uuid);
                            }
                        }

                        // Re-read transport mode from the newly attached control device so
                        // events_transport / events_connected reflect the current firmware state.
                        let new_transport_mode = match tokio::time::timeout(
                            Duration::from_secs(3),
                            control_vm_device.get_transport_mode(),
                        )
                        .await
                        {
                            Ok(Ok(mode)) => {
                                info!(
                                    "AddControlDevice: transport mode for {:02x?} = {:?}",
                                    device.uuid, mode
                                );
                                Some(mode)
                            }
                            Ok(Err(e)) => {
                                warn!(
                                    "AddControlDevice: failed to read transport mode for {:02x?}: {}",
                                    device.uuid, e
                                );
                                None
                            }
                            Err(_) => {
                                warn!(
                                    "AddControlDevice: get_transport_mode timed out for {:02x?}",
                                    device.uuid
                                );
                                None
                            }
                        };
                        if let Some(mode) = new_transport_mode {
                            let is_usb = mode == protodongers::control::device::TransportMode::Usb;
                            device.events_transport = if is_usb {
                                odyssey_hub_common::device::EventsTransport::Wired
                            } else {
                                odyssey_hub_common::device::EventsTransport::Bluetooth
                            };
                            device.events_connected = is_usb;
                            if is_usb {
                                device.capabilities.insert(DeviceCapabilities::EVENTS);
                            } else {
                                device.capabilities.remove(DeviceCapabilities::EVENTS);
                            }
                        }

                        // Store the control device
                        connections.control_device = Some(control_vm_device);

                        // Update device capabilities to include CONTROL
                        device.capabilities.insert(DeviceCapabilities::CONTROL);

                        // Change transport from UsbMux to Usb since USB control is now primary
                        device.transport = odyssey_hub_common::device::Transport::Usb;

                        // Send UpdateDevice to notify clients
                        let _ = message_channel.send(Message::UpdateDevice(device.clone())).await;

                        // Notify clients about the capability change
                        let ev = Event::DeviceEvent(DeviceEvent(
                            device.clone(),
                            DeviceEventKind::CapabilitiesChanged,
                        ));
                        let _ = event_sender.send(ev);

                        info!("USB control device added successfully for {:02x?}, capabilities: {:?}",
                              device.uuid, device.capabilities);
                    }
                    Some(DeviceTaskMessage::RemoveControlDevice) => {
                        info!(
                            "RemoveControlDevice - removing USB control for {:02x?}",
                            device.uuid
                        );

                        connections.control_device = None;
                        device.capabilities.remove(DeviceCapabilities::CONTROL);

                        // A UsbMux device has genuinely independent BLE events that survive
                        // USB disconnect. A Usb-transport device's events are co-located with
                        // the control connection that just disappeared, so always exit.
                        let has_independent_ble_events = connections.events_device.is_some()
                            && device.transport
                                == odyssey_hub_common::device::Transport::UsbMux;

                        if has_independent_ble_events {
                            device.transport = odyssey_hub_common::device::Transport::UsbMux;
                            let _ = message_channel.send(Message::UpdateDevice(device.clone())).await;
                            let ev = Event::DeviceEvent(DeviceEvent(
                                device.clone(),
                                DeviceEventKind::CapabilitiesChanged,
                            ));
                            let _ = event_sender.send(ev);
                            info!(
                                "Device {:02x?} continuing with BLE events only after USB disconnect",
                                device.uuid
                            );
                        } else {
                            // No independent events source — drop any stale wired events device
                            // and exit so the device disappears from the UI.
                            connections.events_device = None;
                            sensor_streams.clear();
                            info!(
                                "Device {:02x?} has no independent events after USB disconnect, exiting",
                                device.uuid
                            );
                            break;
                        }
                    }
                    Some(DeviceTaskMessage::RemoveEventsDevice) => {
                        info!(
                            "RemoveEventsDevice - removing BLE mux events for {:02x?}",
                            device.uuid
                        );

                        // Remove events device
                        connections.events_device = None;

                        // Clear sensor streams
                        sensor_streams.clear();
                        has_sensor_streams = false;

                        // Remove EVENTS capability
                        device.capabilities.remove(DeviceCapabilities::EVENTS);

                        // Mark events as disconnected
                        device.events_connected = false;

                        // If we have control device, update and continue
                        if connections.control_device.is_some() {
                            info!("Device {:02x?} has USB control, continuing without events", device.uuid);

                            // If transport is UsbMux, change it back to Usb since we no longer have BLE events
                            if device.transport == odyssey_hub_common::device::Transport::UsbMux {
                                device.transport = odyssey_hub_common::device::Transport::Usb;
                            }

                            // Send UpdateDevice to notify clients
                            let _ = message_channel.send(Message::UpdateDevice(device.clone())).await;

                            // Notify clients about the capability change
                            let ev = Event::DeviceEvent(DeviceEvent(
                                device.clone(),
                                DeviceEventKind::CapabilitiesChanged,
                            ));
                            let _ = event_sender.send(ev);
                        } else {
                            // No control device, exit
                            info!("Device {:02x?} has no control device, exiting", device.uuid);
                            break;
                        }
                    }
                    None => {
                        info!("Control channel closed for {:02x?}", device.uuid);
                        break;
                    }
                }
            }
            Some(new_delay) = shot_delay_stream.next() => {
                current_shot_delay = new_delay;
                info!(
                    "Shot delay updated for {:02x?} = {} ms",
                    device.uuid, current_shot_delay
                );
                device_shot_delays
                    .write()
                    .await
                    .insert(device.uuid, current_shot_delay as u16);
            }
            maybe_evt = sensor_streams.next(), if has_sensor_streams => {
                match maybe_evt {
                    Some(evt) => {
                        if let Err(e) = handle_stream_event(
                            &device,
                            evt,
                            &event_sender,
                            &device_offsets,
                            &mut fv_state,
                            &screen_calibrations,
                            &mut prev_accel_timestamp,
                            &mut madgwick,
                            &mut orientation,
                            &camera_model_nf,
                            &camera_model_wf,
                            &stereo_iso,
                        ).await {
                            warn!("Error processing stream event for {:02x?}: {}", device.uuid, e);
                        }
                    }
                    None => {
                        info!("Sensor streams ended for device {:02x?}", device.uuid);
                        break;
                    }
                }
            }
            else => {
                warn!("select! else branch fired for {:02x?} — all branches disabled, breaking loop", device.uuid);
                break;
            },
        }
    }

    // Always send Disconnect message when handler ends
    info!(
        "Device handler task ending for {:02x?}, sending disconnect",
        device.uuid
    );
    let disconnect_device = Device {
        uuid: device.uuid,
        transport: device.transport,
        capabilities: device.capabilities.clone(),
        firmware_version: device.firmware_version,
        events_transport: device.events_transport,
        events_connected: device.events_connected,
        product_id: device.product_id,
    };
    let _ = message_channel
        .send(Message::Disconnect(disconnect_device))
        .await;
}

/// Process a single inbound Packet and emit appropriate events.
/// This function emits the same basic events as prior implementation:
/// - Accelerometer -> AccelerometerEvent
/// - Impact -> ImpactEvent
/// - CombinedMarkers/Poc/Object -> TrackingEvent (basic representation)
/// Additionally, it always emits a PacketEvent containing the raw ats_usb vm::Packet
async fn handle_stream_event(
    device: &Device,
    event: DeviceStreamEvent,
    event_sender: &broadcast::Sender<Event>,
    device_offsets: &Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
    fv_state: &mut FoveatedAimpointState,
    screen_calibrations: &Arc<
        ArcSwap<
            ArrayVec<(u8, ScreenCalibration<f32>), { (ats_common::MAX_SCREEN_ID + 1) as usize }>,
        >,
    >,
    prev_accel_timestamp: &mut Option<u32>,
    madgwick: &mut ahrs::Madgwick<f32>,
    orientation: &mut Rotation3<f32>,
    camera_model_nf: &opencv_ros_camera::RosOpenCvIntrinsics<f32>,
    camera_model_wf: &opencv_ros_camera::RosOpenCvIntrinsics<f32>,
    stereo_iso: &Isometry3<f32>,
) -> Result<()> {
    let event_type = match &event {
        DeviceStreamEvent::Accel(_) => "Accel",
        DeviceStreamEvent::Impact(_) => "Impact",
        DeviceStreamEvent::CombinedMarkers(_) => "CombinedMarkers",
        DeviceStreamEvent::PocMarkers(_) => "PocMarkers",
        DeviceStreamEvent::Vendor(_, _) => "Vendor",
    };
    trace!(
        "handle_stream_event for device {:02x?}: {}",
        device.uuid,
        event_type
    );

    match event {
        DeviceStreamEvent::Accel(report) => {
            // Feed IMU into the handler-local foveated state
            let elapsed_us = match prev_accel_timestamp {
                Some(p) => report.timestamp.wrapping_sub(*p) as u64,
                None => 0u64,
            };
            let dt = if elapsed_us == 0 {
                Duration::from_micros(10_000) // default small dt if unknown
            } else {
                Duration::from_micros(elapsed_us)
            };
            // Use same axis ordering as vision module (negated xzy)
            let accel = -report.accel.xzy();
            let gyro = -report.gyro.xzy();
            fv_state.predict(accel, gyro, dt);
            *prev_accel_timestamp = Some(report.timestamp);

            // Update Madgwick orientation filter to compute gravity vector later
            if elapsed_us != 0 {
                let sample_period = madgwick.sample_period_mut();
                *sample_period = elapsed_us as f32 / 1_000_000.0;
            }
            let _ = madgwick.update_imu(
                &Vector3::new(report.gyro.x, report.gyro.y, report.gyro.z),
                &Vector3::new(report.accel.x, report.accel.y, report.accel.z),
            );
            *orientation = madgwick.quat.to_rotation_matrix();

            let ev_kind = DeviceEventKind::AccelerometerEvent(AccelerometerEvent {
                timestamp: report.timestamp,
                accel: report.accel,
                gyro: report.gyro,
                euler_angles: Vector3::zeros(),
            });
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(device.clone(), ev_kind)));

            // Attempt to compute an updated tracking solution using the predicted state.
            let sc = screen_calibrations.load();
            let offset_opt = {
                let guard = device_offsets.lock().await;
                guard.get(&device.uuid).cloned()
            };
            let (pose_opt, aim_and_d_opt) = ats_helpers::raycast_update(&*sc, fv_state, offset_opt);
            if let (Some((rotmat, transvec)), Some((pt, d))) = (pose_opt, aim_and_d_opt) {
                let tracking_pose = transform_pose_for_client(rotmat, transvec);
                let tracking = TrackingEvent {
                    timestamp: report.timestamp,
                    aimpoint: pt.coords,
                    pose: tracking_pose,
                    distance: d,
                    screen_id: fv_state.screen_id as u32,
                };
                let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(
                    device.clone(),
                    DeviceEventKind::TrackingEvent(tracking),
                )));
            }
        }
        DeviceStreamEvent::Impact(report) => {
            let ev_kind = DeviceEventKind::ImpactEvent(ImpactEvent {
                timestamp: report.timestamp,
            });
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(device.clone(), ev_kind)));
        }
        DeviceStreamEvent::CombinedMarkers(report) => {
            let nf_points_filtered: Vec<_> = report
                .nf_points
                .iter()
                .filter(|p| p.x != 0 || p.y != 0)
                .map(|p| Point2::new(p.x as f32, p.y as f32))
                .collect();
            let wf_points_filtered: Vec<_> = report
                .wf_points
                .iter()
                .filter(|p| p.x != 0 || p.y != 0)
                .map(|p| Point2::new(p.x as f32, p.y as f32))
                .collect();

            trace!(
                "CombinedMarkers for {:02x?}: {} NF points, {} WF points",
                device.uuid,
                nf_points_filtered.len(),
                wf_points_filtered.len()
            );

            let nf_markers = build_markers_from_points(nf_points_filtered, camera_model_nf, None);
            let wf_markers =
                build_markers_from_points(wf_points_filtered, camera_model_wf, Some(stereo_iso));

            // Observe markers in the handler-local foveated state and attempt raycast/raycast_update.
            let mut tracking_pose: Option<Pose> = None;
            let mut aimpoint = Vector2::new(0.0f32, 0.0f32);
            let mut distance = 0.0f32;

            let screen_id = {
                let gravity_vec = {
                    let global_down = Vector3::z_axis().into_inner();
                    let local = orientation.inverse_transform_vector(&global_down);
                    let local = Vector3::new(local.x, local.z, local.y);
                    UnitVector3::new_normalize(local)
                };

                // Observe markers directly from existing vectors (pass slices, avoid clones)
                let _observed = fv_state.observe_markers(
                    &nf_markers,
                    &wf_markers,
                    gravity_vec,
                    &*screen_calibrations.load(),
                );

                // Attempt to compute pose and aimpoint using raycast_update.
                // Pass stored device offset (if any) so it affects the solution.
                let sc = screen_calibrations.load();
                let offset_opt = {
                    let guard = device_offsets.lock().await;
                    guard.get(&device.uuid).cloned()
                };
                let (pose_opt, aim_and_d_opt) =
                    ats_helpers::raycast_update(&*sc, fv_state, offset_opt);

                if pose_opt.is_none() {
                    trace!(
                        "raycast_update returned None pose for device {:02x?}. Screen calibrations count: {}, fv_state.screen_id: {}",
                        device.uuid,
                        sc.len(),
                        fv_state.screen_id
                    );
                }

                if let Some((rotmat, transvec)) = pose_opt {
                    tracking_pose = Some(transform_pose_for_client(rotmat, transvec));
                }

                if let Some((pt, d)) = aim_and_d_opt {
                    aimpoint = pt.coords;
                    distance = d;
                }

                fv_state.screen_id as u32
            };

            // If raycast_update didn't produce a pose (and thus no aimpoint), short-circuit.
            let tracking_pose = match tracking_pose {
                Some(p) => p,
                None => {
                    trace!("Raycast failed to produce pose for device {:02x?}, skipping tracking event", device.uuid);
                    return Ok(());
                }
            };

            let tracking = TrackingEvent {
                timestamp: 0,
                aimpoint,
                pose: tracking_pose,
                distance,
                screen_id: screen_id,
            };
            trace!(
                "Sending TrackingEvent for device {:02x?}: aimpoint=({:.3}, {:.3}), distance={:.3}",
                device.uuid,
                aimpoint.x,
                aimpoint.y,
                distance
            );
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(
                device.clone(),
                DeviceEventKind::TrackingEvent(tracking),
            )));
        }
        DeviceStreamEvent::PocMarkers(report) => {
            let nf_markers = build_markers_from_points(
                report
                    .points
                    .iter()
                    .filter(|p| p.x != 0 || p.y != 0)
                    .map(|p| Point2::new(p.x as f32, p.y as f32))
                    .collect(),
                camera_model_nf,
                None,
            );

            let mut tracking_pose: Option<Pose> = None;
            let mut aimpoint = Vector2::new(0.0f32, 0.0f32);
            let mut distance = 0.0f32;

            let screen_id = {
                let gravity_vec = {
                    let global_down = Vector3::z_axis().into_inner();
                    let local = orientation.inverse_transform_vector(&global_down);
                    let local = Vector3::new(local.x, local.z, local.y);
                    UnitVector3::new_normalize(local)
                };
                let wf_empty: Vec<ats_cv::foveated::Marker> = Vec::new();
                let _observed = fv_state.observe_markers(
                    &nf_markers,
                    &wf_empty,
                    gravity_vec,
                    &*screen_calibrations.load(),
                );

                let sc = screen_calibrations.load();
                let offset_opt = {
                    let guard = device_offsets.lock().await;
                    guard.get(&device.uuid).cloned()
                };
                let (pose_opt, aim_and_d_opt) =
                    ats_helpers::raycast_update(&*sc, fv_state, offset_opt);

                if let Some((rotmat, transvec)) = pose_opt {
                    tracking_pose = Some(transform_pose_for_client(rotmat, transvec));
                }
                if let Some((pt, d)) = aim_and_d_opt {
                    aimpoint = pt.coords;
                    distance = d;
                }
                fv_state.screen_id as u32
            };

            // If raycast_update didn't produce a pose (and thus no aimpoint), short-circuit.
            let tracking_pose = match tracking_pose {
                Some(p) => p,
                None => return Ok(()),
            };

            let tracking = TrackingEvent {
                timestamp: 0,
                aimpoint,
                pose: tracking_pose,
                distance,
                screen_id: screen_id,
            };
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(
                device.clone(),
                DeviceEventKind::TrackingEvent(tracking),
            )));
        }
        DeviceStreamEvent::Vendor(tag, vendor) => {
            let vm_pkt = Packet {
                id: 0,
                data: PacketData::Vendor(tag, vendor),
            };
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(
                device.clone(),
                DeviceEventKind::PacketEvent(vm_pkt),
            )));
        }
    }
    Ok(())
}

fn transform_pose_for_client(rotmat: Matrix3<f32>, transvec: Vector3<f32>) -> Pose {
    let flip = Matrix3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0);
    let rotation = flip * rotmat * flip;
    let translation = flip * Matrix3x1::new(transvec.x, transvec.y, transvec.z);
    Pose {
        rotation,
        translation,
    }
}

/// Compute zero offset quaternion from a client-side TrackingEvent.
/// The TrackingEvent pose has been through `transform_pose_for_client` (flip applied),
/// so we undo the flip to recover the raw orientation/position before computing the offset.
fn calculate_zero_offset_from_tracking_event(
    zero_translation: Translation3<f32>,
    zero_target_in_screen_space: Point2<f32>,
    screen_calibrations: &arrayvec::ArrayVec<
        (u8, ats_common::ScreenCalibration<f32>),
        { (ats_common::MAX_SCREEN_ID + 1) as usize },
    >,
    te: &TrackingEvent,
) -> Option<UnitQuaternion<f32>> {
    let screen_id = te.screen_id as u8;
    if screen_id > ats_common::MAX_SCREEN_ID {
        return None;
    }

    // Undo the flip that transform_pose_for_client applied
    let flip = Matrix3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, -1.0);
    let raw_rotation = flip * te.pose.rotation * flip;
    let raw_translation = flip * te.pose.translation;

    let orientation = Rotation3::from_matrix_unchecked(raw_rotation);
    let orientation = UnitQuaternion::from_rotation_matrix(&orientation);
    let position: Vector3<f32> =
        Vector3::new(raw_translation.x, raw_translation.y, raw_translation.z);

    let screen_calibration = ats_helpers::get_screen_calibration(screen_calibrations, screen_id)?;
    let inv_homography = screen_calibration.homography.try_inverse()?;

    let zero_target_2d = inv_homography.transform_point(&zero_target_in_screen_space);
    let zero_target_3d = Point3::new(zero_target_2d.x, zero_target_2d.y, 0.0);
    let iso = Isometry3::from_parts(position.into(), orientation);
    let zero_position = iso * Point3::from(zero_translation.vector);

    let desired_direction = orientation.inverse_transform_vector(&{
        let v = zero_target_3d - zero_position;
        let norm = v.norm();
        if norm.abs() < f32::EPSILON {
            return None;
        }
        v / norm
    });

    let current_forward = Vector3::z();
    UnitQuaternion::rotation_between(&current_forward, &desired_direction)
}

fn build_markers_from_points(
    raw_points: Vec<Point2<f32>>,
    camera_model: &RosOpenCvIntrinsics<f32>,
    stereo_iso: Option<&Isometry3<f32>>,
) -> Vec<ats_cv::foveated::Marker> {
    if raw_points.is_empty() {
        return Vec::new();
    }
    let undistorted = undistort_points(camera_model, &raw_points);
    let intrinsics = ros_opencv_intrinsics_type_convert(camera_model);
    undistorted
        .into_iter()
        .map(|pt| {
            let normalized = to_normalized_image_coordinates(pt, &intrinsics, stereo_iso);
            ats_cv::foveated::Marker {
                position: NaPoint2::new(normalized.x, normalized.y),
            }
        })
        .collect()
}
