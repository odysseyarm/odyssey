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
use odyssey_hub_common::device::Device;
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
#[derive(Debug, Clone)]
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
}

/// Messages emitted by device_tasks to the rest of the system.
pub enum Message {
    Connect(Device, mpsc::Sender<DeviceTaskMessage>),
    Disconnect(Device),
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
    // Map of uuid -> JoinHandle for per-device tasks
    let device_handles = Arc::new(tokio::sync::Mutex::new(HashMap::<
        [u8; 6],
        tokio::task::JoinHandle<()>,
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

            // For ATS Lites (0x5210), check transport mode first - skip if not in USB mode
            if pid == 0x5210 {
                match VmDevice::probe_transport_mode(dev_info).await {
                    Ok(protodongers::control::device::TransportMode::Usb) => {
                        debug!("ATS Lite is in USB mode, will proceed with connection");
                    }
                    Ok(mode) => {
                        debug!("ATS Lite is in {:?} mode (not USB), skipping device", mode);
                        continue;
                    }
                    Err(e) => {
                        debug!("Failed to probe transport mode for ATS Lite: {}", e);
                        continue;
                    }
                }
            }

            if pid == 0x5212 {
                // Hub/dongle (0x5212) - check if we're already managing this hub
                let hub_managers_guard = hub_managers.lock().await;
                let already_managed = hub_managers_guard
                    .iter()
                    .any(|(stored_info, _)| stored_info.id() == dev_info.id());
                drop(hub_managers_guard); // Release lock before connecting

                if already_managed {
                    trace!("Hub already being managed, skipping: {:?}", dev_info.id());
                    continue;
                }

                info!("Attempting to connect to hub/dongle device");
                match ats_usb::device::MuxDevice::connect_usb(dev_info.clone()).await {
                    Ok(hub) => {
                        info!("Connected USB hub device: {:?}", dev_info);

                        // Subscribe to device list changes
                        if let Err(e) = hub.subscribe_device_list().await {
                            tracing::error!("Failed to subscribe to device list: {}", e);
                            continue;
                        }
                        tracing::info!("Subscribed to device list changes");

                        // TODO repeated code
                        // Create device handlers for current hub snapshot
                        match hub.request_devices().await {
                            Ok(list) => {
                                info!("Hub returned {} devices", list.len());
                                for addr in list.iter() {
                                    info!("Processing hub device with BLE address: {:02x?}", addr);
                                    let addr_copy = *addr;

                                    match ats_usb::device::VmDevice::connect_via_mux(
                                        hub.clone(),
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
                                                Ok(protodongers::Props::Uuid(uuid_bytes)) => {
                                                    uuid_bytes
                                                }
                                                Ok(other) => {
                                                    warn!("Expected Uuid prop, got {:?}", other);
                                                    // Fall back to BLE address as UUID
                                                    addr_copy
                                                }
                                                Err(e) => {
                                                    warn!("Failed to read UUID from device: {}", e);
                                                    // Fall back to BLE address as UUID
                                                    addr_copy
                                                }
                                            };

                                            info!(
                                                "Hub device {:02x?} has UUID {:02x?}",
                                                addr_copy, uuid
                                            );

                                            let device_meta = Device {
                                                uuid,
                                                transport:
                                                    odyssey_hub_common::device::Transport::UsbMux,
                                            };
                                            let (ctrl_tx, ctrl_rx) =
                                                mpsc::channel::<DeviceTaskMessage>(32);

                                            let dh_handle = tokio::spawn(device_handler_task(
                                                vm_device,
                                                device_meta.clone(),
                                                ctrl_rx,
                                                screen_calibrations.clone(),
                                                device_offsets.clone(),
                                                device_shot_delays.clone(),
                                                shot_delay_watch.clone(),
                                                event_sender.clone(),
                                            ));

                                            {
                                                let mut dh = device_handles.lock().await;
                                                if !dh.contains_key(&uuid) {
                                                    dh.insert(uuid, dh_handle);
                                                }
                                            }

                                            let _ = message_channel
                                                .send(Message::Connect(device_meta, ctrl_tx))
                                                .await;
                                        }
                                        Err(e) => {
                                            warn!(
                                                "Failed to connect to hub device {:02x?}: {}",
                                                addr_copy, e
                                            );
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                debug!("Failed to request device list from hub: {}", e);
                            }
                        }

                        // Hub manager task: handle device snapshots
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

                            // Request devices again to catch any that connected between the initial
                            // request and this task starting (closes race condition window)
                            match hub_clone.request_devices().await {
                                Ok(list) => {
                                    if !list.is_empty() {
                                        tracing::info!("Hub manager found {} devices on startup, processing...", list.len());
                                        // Send a synthetic snapshot event to process these devices
                                        // This will be handled by the snapshot processing logic below
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "Failed to request devices in hub manager: {}",
                                        e
                                    );
                                }
                            }

                            // Create snapshot stream from hub - will receive DevicesSnapshot
                            // messages automatically when devices connect/disconnect
                            tracing::info!("Creating snapshot stream for hub");
                            let snapshot_stream =
                                stream::unfold(hub_clone.clone(), |hub| async move {
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
                                            tracing::error!(
                                                "receive_snapshot returned Err: {:?}",
                                                e
                                            );
                                            Some((HubEvent::Snapshot(Err(e)), hub))
                                        }
                                    }
                                });

                            tokio::pin!(snapshot_stream);

                            while let Some(event) = snapshot_stream.next().await {
                                match event {
                                    HubEvent::Snapshot(snapshot_result) => {
                                        match snapshot_result {
                                            Ok(devs) => {
                                                tracing::info!("Hub received device snapshot with {} devices: {:02x?}", devs.len(), devs);
                                                use std::collections::HashSet;
                                                let new_set: HashSet<[u8; 6]> =
                                                    devs.into_iter().collect();
                                                let existing_set: HashSet<[u8; 6]> = {
                                                    let dh = device_handles_clone.lock().await;
                                                    dh.keys().cloned().collect()
                                                };

                                                tracing::info!("Device diff - new: {:02x?}, existing: {:02x?}, added: {:02x?}, removed: {:02x?}",
                                                        new_set, existing_set,
                                                        new_set.difference(&existing_set).copied().collect::<Vec<_>>(),
                                                        existing_set.difference(&new_set).copied().collect::<Vec<_>>());

                                                // Added devices
                                                for addr in new_set.difference(&existing_set) {
                                                    let addr_copy = *addr;
                                                    tracing::info!("DevicesSnapshot: Adding new device {:02x?}", addr_copy);

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
                                                                    Ok(protodongers::Props::Uuid(uuid_bytes)) => {
                                                                        uuid_bytes
                                                                    }
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
                                                                    addr_copy, uuid
                                                                );

                                                                let device_meta = Device {
                                                                    uuid,
                                                                    transport: odyssey_hub_common::device::Transport::UsbMux,
                                                                };
                                                                let (ctrl_tx, ctrl_rx) =
                                                                    mpsc::channel::<DeviceTaskMessage>(32);

                                                                let dh_handle =
                                                                    tokio::spawn(device_handler_task(
                                                                        vm_device,
                                                                        device_meta.clone(),
                                                                        ctrl_rx,
                                                                        sc_clone.clone(),
                                                                        doff_clone.clone(),
                                                                        dsd_clone.clone(),
                                                                        swatch_clone.clone(),
                                                                        ev_clone.clone(),
                                                                    ));

                                                                {
                                                                    let mut dh =
                                                                        device_handles_clone.lock().await;
                                                                    if !dh.contains_key(&uuid) {
                                                                        dh.insert(uuid, dh_handle);
                                                                        hub_devices.insert(uuid);
                                                                    }
                                                                }

                                                                let _ = message_channel_clone
                                                                    .send(Message::Connect(
                                                                        device_meta,
                                                                        ctrl_tx,
                                                                    ))
                                                                    .await;
                                                            }
                                                            Err(e) => {
                                                                tracing::warn!(
                                                                    "Failed to connect to hub device {:02x?} during snapshot: {}",
                                                                    addr_copy, e
                                                                );
                                                            }
                                                        }
                                                }

                                                // Removed devices
                                                let to_remove: Vec<[u8; 6]> = {
                                                    let dh = device_handles_clone.lock().await;
                                                    dh.keys()
                                                        .cloned()
                                                        .filter(|a| !new_set.contains(a))
                                                        .collect()
                                                };
                                                for addr in to_remove {
                                                    let uuid = addr;
                                                    tracing::info!(
                                                        "DevicesSnapshot: Removing device {:02x?}",
                                                        uuid
                                                    );
                                                    if let Some(handle) = device_handles_clone
                                                        .lock()
                                                        .await
                                                        .remove(&uuid)
                                                    {
                                                        handle.abort();
                                                        hub_devices.remove(&uuid);
                                                    }
                                                    let device_meta = Device {
                                                            uuid,
                                                            transport: odyssey_hub_common::device::Transport::UsbMux,
                                                        };
                                                    let _ = message_channel_clone
                                                        .send(Message::Disconnect(device_meta))
                                                        .await;
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

                            // Cleanup: remove all devices managed by this hub
                            tracing::info!(
                                "Hub manager task ending for hub {:?}, cleaning up {} devices",
                                hub_info,
                                hub_devices.len()
                            );
                            for uuid in hub_devices {
                                if let Some(handle) =
                                    device_handles_clone.lock().await.remove(&uuid)
                                {
                                    handle.abort();
                                    tracing::info!("Aborted device handler for {:02x?}", uuid);
                                }
                                let device_meta = Device {
                                    uuid,
                                    transport: odyssey_hub_common::device::Transport::UsbMux,
                                };
                                let _ = message_channel_clone
                                    .send(Message::Disconnect(device_meta))
                                    .await;
                            }
                            tracing::info!("Hub manager task ended for hub {:?}", hub_info);
                        });

                        // Store hub manager handle for tracking (runs indefinitely until hub is disconnected)
                        {
                            let mut hm = hub_managers.lock().await;
                            // Store with a dummy slot - we track the handle itself
                            let hub_slot = std::sync::Arc::new(tokio::sync::Mutex::new(Some(
                                hub_manager_handle,
                            )));
                            hm.push((dev_info.clone(), hub_slot));
                        }
                    }
                    Err(e) => {
                        warn!("Failed to connect to USB hub device: {}", e);
                    }
                }
            } else {
                // Direct USB device (VmDevice) - 0x520F, 0x5210 (USB mode only), or 0x5211
                info!(
                    "Attempting to connect to direct USB device PID: 0x{:04x}",
                    pid
                );
                match VmDevice::connect_usb(dev_info.clone()).await {
                    Ok(vm_device) => {
                        let uuid = match vm_device.read_prop(protodongers::PropKind::Uuid).await {
                            Ok(protodongers::Props::Uuid(bytes)) => bytes,
                            Ok(other) => {
                                warn!("Expected Uuid prop, got {:?}, skipping device", other);
                                continue;
                            }
                            Err(e) => {
                                warn!("Failed to read UUID from USB device: {}", e);
                                continue;
                            }
                        };

                        let mut dh = device_handles.lock().await;
                        if dh.contains_key(&uuid) {
                            info!("Device {:02x?} already in device_handles, skipping", uuid);
                            continue;
                        }

                        info!("Discovered direct USB device with UUID {:02x?}", uuid);

                        let device_meta = Device {
                            uuid,
                            transport: odyssey_hub_common::device::Transport::Usb,
                        };

                        let (tx, rx) = mpsc::channel(32);

                        let handle = tokio::spawn(device_handler_task(
                            vm_device,
                            device_meta.clone(),
                            rx,
                            screen_calibrations.clone(),
                            device_offsets.clone(),
                            device_shot_delays.clone(),
                            shot_delay_watch.clone(),
                            event_sender.clone(),
                        ));

                        dh.insert(uuid, handle);

                        let _ = message_channel
                            .send(Message::Connect(device_meta, tx))
                            .await;
                    }
                    Err(e) => {
                        debug!("Failed to connect to USB device: {}", e);
                    }
                }
            }
        }

        // Retain only running device tasks and send disconnect messages for finished tasks
        {
            let mut dh = device_handles.lock().await;
            let mut finished_devices = Vec::new();

            dh.retain(|uuid, handle| {
                if handle.is_finished() {
                    info!("Device {:02x?} task finished", uuid);
                    finished_devices.push(*uuid);
                    false
                } else {
                    true
                }
            });

            drop(dh); // Release lock before sending messages

            // Send disconnect messages for all finished devices
            for uuid in finished_devices {
                let device_meta = Device {
                    uuid,
                    transport: odyssey_hub_common::device::Transport::Usb,
                };
                let _ = message_channel.send(Message::Disconnect(device_meta)).await;
            }
        }

        sleep(Duration::from_secs(2)).await;
    }
}

/// Per-device handler: receives packets from the link, processes control messages,
/// watches for shot-delay changes, and emits events.
async fn device_handler_task(
    vm_device: VmDevice,
    device: Device,
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

    let general_settings = match vm_device.read_all_config().await {
        Ok(cfg) => cfg,
        Err(e) => {
            warn!(
                "Failed to read camera config for device {:02x?}: {}",
                device.uuid, e
            );
            return;
        }
    };
    let camera_model_nf = general_settings.camera_model_nf.clone();
    let camera_model_wf = general_settings.camera_model_wf.clone();
    let stereo_iso = general_settings.stereo_iso.clone();

    // Start sensor streams via VmDevice helpers so each stream reserves a slot
    let mut sensor_streams: SelectAll<BoxStream<'static, DeviceStreamEvent>> = SelectAll::new();

    match vm_device.clone().stream_combined_markers().await {
        Ok(stream) => {
            info!("Combined markers stream started for {:02x?}", device.uuid);
            sensor_streams.push(stream.map(DeviceStreamEvent::CombinedMarkers).boxed());
        }
        Err(e) => warn!(
            "Failed to start combined markers stream for {:02x?}: {}",
            device.uuid, e
        ),
    }
    match vm_device.clone().stream_poc_markers().await {
        Ok(stream) => {
            info!("PoC markers stream started for {:02x?}", device.uuid);
            sensor_streams.push(stream.map(DeviceStreamEvent::PocMarkers).boxed());
        }
        Err(e) => warn!(
            "Failed to start PoC markers stream for {:02x?}: {}",
            device.uuid, e
        ),
    }
    match vm_device.clone().stream_accel().await {
        Ok(stream) => {
            info!("Accelerometer stream started for {:02x?}", device.uuid);
            sensor_streams.push(stream.map(DeviceStreamEvent::Accel).boxed());
        }
        Err(e) => warn!(
            "Failed to start accelerometer stream for {:02x?}: {}",
            device.uuid, e
        ),
    }
    match vm_device.clone().stream_impact().await {
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

    let mut sensor_stream = sensor_streams.fuse();
    let mut shot_delay_stream = tokio_stream::wrappers::WatchStream::new(shot_delay_rx).fuse();
    let mut control_stream = tokio_stream::wrappers::ReceiverStream::new(control_rx).fuse();

    info!(
        "Device handler task entering main loop for {:02x?}",
        device.uuid
    );

    loop {
        tokio::select! {
            maybe_evt = sensor_stream.next() => {
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
                        if let Err(e) = vm_device
                            .write_vendor(tag, &data[..clipped_len])
                            .await
                        {
                            warn!("Failed to send vendor packet: {}", e);
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
                    None => {
                        info!("Control channel closed for {:02x?}", device.uuid);
                        break;
                    }
                }
            }
            else => break,
        }
    }

    info!("Device handler task ending for {:02x?}", device.uuid);
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
            let nf_markers = build_markers_from_points(
                report
                    .nf_points
                    .iter()
                    .filter(|p| p.x != 0 || p.y != 0)
                    .map(|p| Point2::new(p.x as f32, p.y as f32))
                    .collect(),
                camera_model_nf,
                None,
            );
            let wf_markers = build_markers_from_points(
                report
                    .wf_points
                    .iter()
                    .filter(|p| p.x != 0 || p.y != 0)
                    .map(|p| Point2::new(p.x as f32, p.y as f32))
                    .collect(),
                camera_model_wf,
                Some(stereo_iso),
            );

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
