use ahrs::Ahrs;
use anyhow::Result;
use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use ats_common::{ros_opencv_intrinsics_type_convert, ScreenCalibration};
use ats_usb::device::VmDevice;
use ats_usb::packets::vm::{
    AccelReport, CombinedMarkersReport, ImpactReport, Packet, PacketData, PocMarkersReport,
    VendorData,
};
use nalgebra::{
    Isometry3, Matrix3, Matrix3x1, Point2, Rotation3, Translation3, UnitQuaternion, UnitVector3,
    Vector2, Vector3,
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
    Zero(nalgebra::Translation3<f32>, nalgebra::Point2<f32>),
    ClearZero,
    /// Add an events device to enable sensor streams (for merging USB direct + BLE mux)
    AddEventsDevice(VmDevice),
    /// Add a control device to enable control operations (for merging BLE mux + USB direct BLE mode)
    AddControlDevice(VmDevice),
    /// Remove events device when BLE mux disconnects
    RemoveEventsDevice,
}

/// Device connection channels
pub struct DeviceChannels {
    /// Control channel (only if device has CONTROL capability)
    pub control: Option<mpsc::Sender<protodongers::control::device::DeviceMsg>>,
    /// Commands channel for event-stream related operations (only if device has EVENTS capability)
    pub commands: Option<mpsc::Sender<DeviceTaskMessage>>,
}

/// Messages emitted by device_tasks to the rest of the system.
pub enum Message {
    Connect(Device, DeviceChannels),
    Disconnect(Device),
    UpdateDevice(Device), // Update device metadata (e.g., capabilities changed)
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

    // Hub managers: Vec of (DeviceInfo, slot-with-joinhandle)
    let hub_managers = Arc::new(tokio::sync::Mutex::new(Vec::<(
        nusb::DeviceInfo,
        std::sync::Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    )>::new()));

    loop {
        let devices = match nusb::list_devices().await {
            Ok(iter) => iter.collect::<Vec<_>>(),
            Err(e) => {
                warn!("Failed to list USB devices: {}", e);
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

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

                    while let Some(event) = combined_stream.next().await {
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
                                                                        Some([v.firmware_semver[0], v.firmware_semver[1], v.firmware_semver[2]])
                                                                    }
                                                                    Ok(other) => {
                                                                        tracing::warn!("Unexpected response reading version from BLE device {:02x?}: {:?}", uuid, other);
                                                                        None
                                                                    }
                                                                    Err(e) => {
                                                                        tracing::warn!("Failed to read version from BLE device {:02x?}: {}", uuid, e);
                                                                        None
                                                                    }
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

                                                        // BLE mux devices only have commands channel (no control)
                                                        let channels = DeviceChannels {
                                                            control: None,
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
                    }

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
                debug!(
                    "Found direct USB device PID: 0x{:04x}, checking if already connected",
                    pid
                );

                // Connect and read UUID/version/transport mode first
                // We'll keep this connection if we decide to use it
                let (uuid, version, transport_mode, vm_device_temp) = {
                    match VmDevice::connect_usb_any_mode(dev_info.clone()).await {
                        Ok(vm_device) => {
                            // Read UUID - use control plane for devices that support it (ATS Lite), packets for others
                            let uuid = if pid == 0x5210 {
                                // ATS Lite - use control plane (works in any transport mode)
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

                // Now check if device is already connected BEFORE creating a new connection
                let should_connect = {
                    let mut dh = device_handles.lock().await;
                    info!(
                        "Checking if device {:02x?} is already connected (current handles: {})",
                        uuid,
                        dh.len()
                    );

                    // Check if device is already connected
                    if let Some((existing_cmd_tx, _existing_handle)) = dh.get(&uuid) {
                        // Device already exists - try to add USB control
                        info!(
                            "Device {:02x?} already connected, sending AddControlDevice to merge USB control",
                            uuid
                        );

                        // Send AddControlDevice message to the existing handler
                        let result = existing_cmd_tx
                            .send(DeviceTaskMessage::AddControlDevice(vm_device_temp.clone()))
                            .await;
                        info!(
                            "AddControlDevice message sent for {:02x?}, result: {:?}",
                            uuid, result
                        );

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
                    firmware_version: version.as_ref().map(|v| {
                        [
                            v.firmware_semver[0],
                            v.firmware_semver[1],
                            v.firmware_semver[2],
                        ]
                    }),
                    events_transport: if transport_mode
                        == protodongers::control::device::TransportMode::Usb
                    {
                        odyssey_hub_common::device::EventsTransport::Wired
                    } else {
                        odyssey_hub_common::device::EventsTransport::Bluetooth
                    },
                    events_connected: transport_mode
                        == protodongers::control::device::TransportMode::Usb,
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
                }

                // USB direct devices always have control, but only have commands if in USB transport mode
                let channels = DeviceChannels {
                    control: None, // TODO: Add control channel when control protocol is integrated
                    commands: if transport_mode == protodongers::control::device::TransportMode::Usb
                    {
                        Some(tx)
                    } else {
                        // BLE mode: no events, so no commands channel needed for client
                        // Keep tx alive by spawning a task that holds it until cancelled
                        tokio::spawn(async move {
                            let _tx = tx; // Keep channel open
                            std::future::pending::<()>().await
                        });
                        None
                    },
                };

                let _ = message_channel
                    .send(Message::Connect(device_meta, channels))
                    .await;
            }
        }

        // Detect removed direct USB devices and abort their handlers (similar to hub removal logic)
        // We need to track USB device info alongside handles to properly detect removal
        {
            let mut dh = device_handles.lock().await;
            let mut to_remove_uuids = Vec::<[u8; 6]>::new();

            // Only check finished handles for now
            // USB enumeration-based removal will be handled when we track DeviceInfo alongside handles
            for (uuid, (_cmd_tx, handle)) in dh.iter() {
                // If handle is finished, mark for removal
                if handle.is_finished() {
                    info!("Device {:02x?} task finished", uuid);
                    to_remove_uuids.push(*uuid);
                }
            }

            // Remove finished devices
            for uuid in to_remove_uuids {
                if let Some((_cmd_tx, handle)) = dh.remove(&uuid) {
                    handle.abort();
                    info!("Removed finished device handler for {:02x?}", uuid);
                    // Note: Device handler should have sent Disconnect message before exiting
                }
            }
        }

        sleep(Duration::from_secs(2)).await;
    }
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

    // Read camera config if we have an events device (USB direct or BLE mux)
    // Only skip for USB devices in BLE transport mode (no events device yet)
    let mut general_settings = if let Some(events_device) = connections.events_device.as_ref() {
        info!(
            "Reading camera config for device {:02x?} from events device",
            device.uuid
        );
        match events_device.read_all_config().await {
            Ok(cfg) => {
                info!(
                    "Successfully read camera config for device {:02x?}",
                    device.uuid
                );
                Some(cfg)
            }
            Err(e) => {
                warn!(
                    "Failed to read camera config for device {:02x?}: {}",
                    device.uuid, e
                );
                None
            }
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
        match events_device.clone().stream_combined_markers().await {
            Ok(stream) => {
                info!("Combined markers stream started for {:02x?}", device.uuid);
                sensor_streams.push(stream.map(DeviceStreamEvent::CombinedMarkers).boxed());
            }
            Err(e) => warn!(
                "Failed to start combined markers stream for {:02x?}: {}",
                device.uuid, e
            ),
        }
        match events_device.clone().stream_poc_markers().await {
            Ok(stream) => {
                info!("PoC markers stream started for {:02x?}", device.uuid);
                sensor_streams.push(stream.map(DeviceStreamEvent::PocMarkers).boxed());
            }
            Err(e) => warn!(
                "Failed to start PoC markers stream for {:02x?}: {}",
                device.uuid, e
            ),
        }
        match events_device.clone().stream_accel().await {
            Ok(stream) => {
                info!("Accelerometer stream started for {:02x?}", device.uuid);
                sensor_streams.push(stream.map(DeviceStreamEvent::Accel).boxed());
            }
            Err(e) => warn!(
                "Failed to start accelerometer stream for {:02x?}: {}",
                device.uuid, e
            ),
        }
        match events_device.clone().stream_impact().await {
            Ok(stream) => {
                info!("Impact stream started for {:02x?}", device.uuid);
                sensor_streams.push(stream.map(DeviceStreamEvent::Impact).boxed());
            }
            Err(e) => warn!(
                "Failed to start impact stream for {:02x?}: {}",
                device.uuid, e
            ),
        }

        // TODO separate subscribes to whichever vendor streams we want
        // for tag in 0x81u8..=0xfeu8 {
        //     match vm_device.clone().stream(PacketType::Vendor(tag)).await {
        //         Ok(stream) => {
        //             sensor_streams.push(
        //                 stream
        //                     .filter_map(|pd| async move {
        //                         if let PacketData::Vendor(vtag, vdata) = pd {
        //                             Some(DeviceStreamEvent::Vendor(vtag, vdata))
        //                         } else {
        //                             None
        //                         }
        //                     })
        //                     .boxed(),
        //             );
        //         }
        //         Err(_e) => {
        //             // it's fine if certain vendor streams aren't available
        //         }
        //     }
        // }
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

    // For devices with USB control, periodically check USB connection
    // BLE mux devices (without USB control) don't need health checks - their connectivity is managed through the hub
    let mut needs_health_check = connections.control_device.is_some();
    let mut health_check = if needs_health_check {
        tokio::time::interval(Duration::from_secs(1))
    } else {
        tokio::time::interval(Duration::from_secs(3600)) // Never tick if we don't need health checks
    };

    info!(
        "Device handler task entering main loop for {:02x?}",
        device.uuid
    );

    loop {
        tokio::select! {
            _ = health_check.tick() => {
                // For direct USB devices without sensor streams (BLE mode), check if USB is still connected
                if needs_health_check {
                    debug!("Health check: checking USB connection for device {:02x?}", device.uuid);
                    // Try to read version to check if USB control is still working
                    if let Some(control_dev) = connections.control_device.as_ref() {
                        match control_dev.read_version().await {
                            Ok(_) => {
                                debug!("Health check: device {:02x?} still connected", device.uuid);
                            }
                            Err(e) => {
                                info!("USB control disconnected for device {:02x?} (control read failed: {})", device.uuid, e);

                                // If we have events device (BLE mux), keep running without control
                                if connections.events_device.is_some() {
                                    info!("Device {:02x?} has BLE mux events, removing control and continuing", device.uuid);

                                    // Remove control device
                                    connections.control_device = None;

                                    // Remove CONTROL capability
                                    device.capabilities.remove(DeviceCapabilities::CONTROL);

                                    // Disable health check since we no longer have USB control
                                    needs_health_check = false;

                                    // Change transport from Usb to UsbMux since we only have BLE events now
                                    device.transport = odyssey_hub_common::device::Transport::UsbMux;

                                    // Send UpdateDevice to notify clients
                                    let _ = message_channel.send(Message::UpdateDevice(device.clone())).await;

                                    info!("Device {:02x?} continuing with BLE mux events only", device.uuid);
                                } else {
                                    // No events device, exit completely
                                    info!("Device {:02x?} has no events device, exiting", device.uuid);
                                    break;
                                }
                            }
                        }
                    } else {
                        // No control device for health check
                        if connections.events_device.is_none() {
                            // No control and no events, exit
                            warn!("Health check requested but no control or events device for {:02x?}", device.uuid);
                            break;
                        }
                        // Has events device, just disable health check
                        needs_health_check = false;
                    }
                }
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
            maybe_msg = control_stream.next() => {
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
                    Some(DeviceTaskMessage::Zero(trans, _point)) => {
                        debug!(
                            "Zero received - storing device offset for {:02x?}",
                            device.uuid
                        );
                        let iso = Isometry3::from_parts(
                            Translation3::from(trans.vector),
                            UnitQuaternion::identity(),
                        );
                        {
                            let mut guard = device_offsets.lock().await;
                            guard.insert(device.uuid, iso);
                        }
                        let ev = Event::DeviceEvent(DeviceEvent(
                            device.clone(),
                            DeviceEventKind::ZeroResult(true),
                        ));
                        let _ = event_sender.send(ev);
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
                    Some(DeviceTaskMessage::AddEventsDevice(events_vm_device)) => {
                        info!(
                            "AddEventsDevice - adding BLE mux events device for {:02x?}",
                            device.uuid
                        );

                        // Store the events device
                        connections.events_device = Some(events_vm_device.clone());

                        // Start all sensor streams with the new events device
                        match events_vm_device.clone().stream_combined_markers().await {
                            Ok(stream) => {
                                info!("Combined markers stream started for {:02x?}", device.uuid);
                                sensor_streams.push(stream.map(DeviceStreamEvent::CombinedMarkers).boxed());
                            }
                            Err(e) => warn!("Failed to start combined markers stream for {:02x?}: {}", device.uuid, e),
                        }
                        match events_vm_device.clone().stream_poc_markers().await {
                            Ok(stream) => {
                                info!("PoC markers stream started for {:02x?}", device.uuid);
                                sensor_streams.push(stream.map(DeviceStreamEvent::PocMarkers).boxed());
                            }
                            Err(e) => warn!("Failed to start PoC markers stream for {:02x?}: {}", device.uuid, e),
                        }
                        match events_vm_device.clone().stream_accel().await {
                            Ok(stream) => {
                                info!("Accelerometer stream started for {:02x?}", device.uuid);
                                sensor_streams.push(stream.map(DeviceStreamEvent::Accel).boxed());
                            }
                            Err(e) => warn!("Failed to start accelerometer stream for {:02x?}: {}", device.uuid, e),
                        }
                        match events_vm_device.clone().stream_impact().await {
                            Ok(stream) => {
                                info!("Impact stream started for {:02x?}", device.uuid);
                                sensor_streams.push(stream.map(DeviceStreamEvent::Impact).boxed());
                            }
                            Err(e) => warn!("Failed to start impact stream for {:02x?}: {}", device.uuid, e),
                        }

                        // Update has_sensor_streams flag
                        has_sensor_streams = !sensor_streams.is_empty();

                        // Update device config from BLE mux device
                        // This is critical when a device starts in BLE mode (no config)
                        // and later gets BLE mux events added
                        info!("Reading device config from BLE mux device for {:02x?}", device.uuid);
                        match events_vm_device.read_all_config().await {
                            Ok(cfg) => {
                                info!("Updating device config for {:02x?} from BLE mux", device.uuid);
                                // Update camera models from the new config
                                camera_model_nf = cfg.camera_model_nf.clone();
                                camera_model_wf = cfg.camera_model_wf.clone();
                                stereo_iso = cfg.stereo_iso.clone();
                                // Store the full config for future use
                                general_settings = Some(cfg);
                            }
                            Err(e) => {
                                warn!("Failed to read device config from BLE mux {:02x?}: {}", device.uuid, e);
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

                        // Read firmware version from USB control device if not already available
                        if device.firmware_version.is_none() {
                            match control_vm_device.read_version().await {
                                Ok(v) => {
                                    info!(
                                        "Read firmware version from USB control for {:02x?}: {}.{}.{}",
                                        device.uuid, v.firmware_semver[0], v.firmware_semver[1], v.firmware_semver[2]
                                    );
                                    device.firmware_version = Some([
                                        v.firmware_semver[0],
                                        v.firmware_semver[1],
                                        v.firmware_semver[2],
                                    ]);
                                }
                                Err(e) => {
                                    warn!("Failed to read firmware version from USB control for {:02x?}: {}", device.uuid, e);
                                }
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

                        // Enable health check since we now have USB control that needs monitoring
                        needs_health_check = true;
                        health_check = tokio::time::interval(Duration::from_secs(1));

                        info!("USB control device added successfully for {:02x?}, capabilities: {:?}",
                              device.uuid, device.capabilities);
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
            else => break,
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
