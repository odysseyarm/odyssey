use anyhow::Result;
use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use ats_common::ScreenCalibration;
use nalgebra::{Isometry3, Matrix3x1, Translation3, UnitQuaternion, UnitVector3, Vector2, Vector3};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::{
    AccelerometerEvent, DeviceEvent, DeviceEventKind, Event, ImpactEvent, Pose, TrackingEvent,
};
use protodongers::{Packet, PacketData, VendorData};
use std::collections::HashMap;

use futures::stream::{self, StreamExt};
use futures_concurrency::prelude::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch, Mutex, RwLock};
use tokio::time::sleep;
use tracing::{debug, info, trace, warn};

use crate::device_link::DeviceLink;
use crate::usb_link::UsbLink;

/// Events from the hub manager's merged stream
enum HubEvent {
    Snapshot(Result<heapless::Vec<[u8; 6], 3>>),
}

/// Events from the device handler's merged stream
enum DeviceHandlerEvent {
    Packet(Result<Packet, anyhow::Error>),
    ShotDelayChanged(u32),
    ControlMessage(Option<DeviceTaskMessage>),
}

// ats_cv foveated helpers
use ats_cv::foveated::FoveatedAimpointState;
use ats_cv::helpers as ats_helpers;
use nalgebra::Point2 as NaPoint2;

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
                        && d.product_id() == 0x5210
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
            if dev_info.vendor_id() == 0x1915
                && matches!(dev_info.product_id(), 0x520f | 0x5210 | 0x5211)
            {
                let pid = dev_info.product_id();
                if pid == 0x5210 {
                    // Hub/dongle - check if we're already managing this hub
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

                            // Create device handlers for current hub snapshot
                            match hub.request_devices().await {
                                Ok(list) => {
                                    info!("Hub returned {} devices", list.len());
                                    for addr in list.iter() {
                                        info!(
                                            "Processing hub device with BLE address: {:02x?}",
                                            addr
                                        );
                                        let addr_copy = *addr;

                                        // Use VmDevice::connect_via_hub for proper device abstraction
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
                                                        warn!(
                                                            "Expected Uuid prop, got {:?}",
                                                            other
                                                        );
                                                        // Fall back to BLE address as UUID
                                                        addr_copy
                                                    }
                                                    Err(e) => {
                                                        warn!(
                                                            "Failed to read UUID from device: {}",
                                                            e
                                                        );
                                                        // Fall back to BLE address as UUID
                                                        addr_copy
                                                    }
                                                };

                                                info!(
                                                    "Hub device {:02x?} has UUID {:02x?}",
                                                    addr_copy, uuid
                                                );

                                                // Create UsbLink from VmDevice with UsbHub transport
                                                // This will properly handle streaming via VmDevice
                                                let link =
                                                    match crate::usb_link::UsbLink::from_vm_device(
                                                        vm_device, uuid,
                                                    )
                                                    .await
                                                    {
                                                        Ok(link) => link,
                                                        Err(e) => {
                                                            warn!("Failed to create UsbLink from VmDevice: {}", e);
                                                            continue;
                                                        }
                                                    };
                                                let (ctrl_tx, ctrl_rx) =
                                                    mpsc::channel::<DeviceTaskMessage>(32);

                                                let dh_handle = tokio::spawn(device_handler_task(
                                                    Box::new(link),
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

                                                let device_meta = Device {
                                                    uuid,
                                                    transport:
                                                        odyssey_hub_common::device::Transport::UsbHub,
                                                };
                                                let _ = message_channel
                                                    .send(Message::Connect(
                                                        device_meta.clone(),
                                                        ctrl_tx,
                                                    ))
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

                                // Subscribe to device list changes - this will send an initial
                                // DevicesSnapshot and then send updates whenever devices connect or disconnect
                                if let Err(e) = hub_clone.subscribe_device_list().await {
                                    tracing::error!("Failed to subscribe to device list: {}", e);
                                    return;
                                }
                                tracing::info!("Subscribed to device list changes");

                                // Create snapshot stream from hub - will receive DevicesSnapshot
                                // messages automatically when devices connect/disconnect
                                let snapshot_stream =
                                    stream::unfold(hub_clone.clone(), |hub| async move {
                                        match hub.receive_snapshot().await {
                                            Ok(devs) => Some((HubEvent::Snapshot(Ok(devs)), hub)),
                                            Err(e) => Some((HubEvent::Snapshot(Err(e)), hub)),
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

                                                                // Create UsbLink from VmDevice with UsbHub transport
                                                                let link =
                                                                    match crate::usb_link::UsbLink::from_vm_device(
                                                                        vm_device, uuid,
                                                                    )
                                                                    .await
                                                                    {
                                                                        Ok(link) => link,
                                                                        Err(e) => {
                                                                            tracing::warn!("Failed to create UsbLink from VmDevice: {}", e);
                                                                            continue;
                                                                        }
                                                                    };

                                                                let (ctrl_tx, ctrl_rx) =
                                                                    mpsc::channel::<DeviceTaskMessage>(32);

                                                                let dh_handle =
                                                                    tokio::spawn(device_handler_task(
                                                                        Box::new(link),
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
                                                                    }
                                                                }

                                                                let device_meta = Device {
                                                                    uuid,
                                                                    transport: odyssey_hub_common::device::Transport::UsbHub,
                                                                };
                                                                let _ = message_channel_clone
                                                                    .send(Message::Connect(
                                                                        device_meta.clone(),
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
                                                        }
                                                        let device_meta = Device {
                                                            uuid,
                                                            transport: odyssey_hub_common::device::Transport::UsbHub,
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
                    // Direct USB device (VmDevice)
                    info!(
                        "Attempting to connect to direct USB device PID: 0x{:04x}",
                        pid
                    );
                    match UsbLink::connect_usb(dev_info.clone()).await {
                        Ok(link) => {
                            let device = link.device();
                            let uuid = device.uuid;

                            {
                                let mut dh = device_handles.lock().await;
                                if dh.contains_key(&uuid) {
                                    info!(
                                        "Device {:02x?} already in device_handles, skipping",
                                        uuid
                                    );
                                    continue;
                                }

                                info!("Discovered direct USB device with UUID {:02x?}", uuid);

                                let (tx, rx) = mpsc::channel(32);

                                let handle = tokio::spawn(device_handler_task(
                                    Box::new(link),
                                    rx,
                                    screen_calibrations.clone(),
                                    device_offsets.clone(),
                                    device_shot_delays.clone(),
                                    shot_delay_watch.clone(),
                                    event_sender.clone(),
                                ));

                                dh.insert(uuid, handle);

                                let _ = message_channel
                                    .send(Message::Connect(device.clone(), tx))
                                    .await;
                            }
                        }
                        Err(e) => {
                            debug!("Failed to connect to USB device: {}", e);
                        }
                    }
                }
            }
        }

        // Retain only running device tasks
        {
            let mut dh = device_handles.lock().await;
            dh.retain(|uuid, handle| {
                if handle.is_finished() {
                    info!("Device {:02x?} task finished", uuid);
                    false
                } else {
                    true
                }
            });
        }

        sleep(Duration::from_secs(2)).await;
    }
}

/// Per-device handler: receives packets from the link, processes control messages,
/// watches for shot-delay changes, and emits events.
async fn device_handler_task(
    mut link: Box<dyn DeviceLink>,
    mut control_rx: mpsc::Receiver<DeviceTaskMessage>,
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
    let device = link.device();
    info!(
        "Starting device handler for device UUID {:02x?}",
        device.uuid
    );

    // Ensure shot-delay watch exists and subscribe
    let mut shot_delay_rx = {
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

    // Enable device streams by sending StreamUpdate packets
    use protodongers::{PacketType, StreamUpdate, StreamUpdateAction};
    info!("Enabling streams for device {:02x?}", device.uuid);

    // Enable accelerometer stream
    if let Err(e) = link
        .send(Packet {
            id: 0,
            data: PacketData::StreamUpdate(StreamUpdate {
                packet_id: PacketType::AccelReport(),
                action: StreamUpdateAction::Enable,
            }),
        })
        .await
    {
        warn!("Failed to enable AccelReport stream: {}", e);
    }

    // Enable combined markers stream
    if let Err(e) = link
        .send(Packet {
            id: 0,
            data: PacketData::StreamUpdate(StreamUpdate {
                packet_id: PacketType::CombinedMarkersReport(),
                action: StreamUpdateAction::Enable,
            }),
        })
        .await
    {
        warn!("Failed to enable CombinedMarkersReport stream: {}", e);
    }

    // Enable impact stream
    if let Err(e) = link
        .send(Packet {
            id: 0,
            data: PacketData::StreamUpdate(StreamUpdate {
                packet_id: PacketType::ImpactReport(),
                action: StreamUpdateAction::Enable,
            }),
        })
        .await
    {
        warn!("Failed to enable ImpactReport stream: {}", e);
    }

    info!(
        "Stream initialization complete for device {:02x?}",
        device.uuid
    );

    // Wrap link in Arc<Mutex> so it can be shared between streams
    let link = Arc::new(Mutex::new(link));

    // Convert packet stream
    let link_clone = link.clone();
    let packet_stream = stream::unfold(link_clone, |link| async move {
        let mut guard = link.lock().await;
        let result = guard.recv().await;
        drop(guard);
        Some((DeviceHandlerEvent::Packet(result), link))
    });

    // Convert shot delay watcher to stream
    let shot_delay_stream = tokio_stream::wrappers::WatchStream::new(shot_delay_rx)
        .map(|val| DeviceHandlerEvent::ShotDelayChanged(val));

    // Convert control message receiver to stream
    // We map to Option<DeviceTaskMessage> and handle None in the loop
    let control_stream = tokio_stream::wrappers::ReceiverStream::new(control_rx)
        .map(|msg| DeviceHandlerEvent::ControlMessage(Some(msg)));

    // Merge all three streams
    // Note: merge() continues until ALL streams end, not when one ends
    let merged = (packet_stream, shot_delay_stream, control_stream).merge();
    tokio::pin!(merged);

    while let Some(event) = merged.next().await {
        match event {
            DeviceHandlerEvent::Packet(pkt_result) => match pkt_result {
                Ok(pkt) => {
                    if let Err(e) = process_packet(
                        &device,
                        pkt.clone(),
                        &event_sender,
                        &device_offsets,
                        &mut fv_state,
                        &screen_calibrations,
                        &mut prev_accel_timestamp,
                    )
                    .await
                    {
                        warn!("Error processing packet: {}", e);
                    }
                }
                Err(e) => {
                    warn!("Device recv error: {}", e);
                    break;
                }
            },

            DeviceHandlerEvent::ShotDelayChanged(new_delay) => {
                current_shot_delay = new_delay;
                info!(
                    "Shot delay updated for {:02x?} = {} ms",
                    device.uuid, current_shot_delay
                );
                // Update in-memory store as well (keeps server and task in sync)
                device_shot_delays
                    .write()
                    .await
                    .insert(device.uuid, current_shot_delay as u16);
                // Do not emit a ShotDelayChanged DeviceEvent here — the server RPCs
                // and other code paths are responsible for broadcasting higher-level updates.
            }

            DeviceHandlerEvent::ControlMessage(msg) => {
                match msg {
                    Some(DeviceTaskMessage::ResetZero) => {
                        debug!("ResetZero requested for {:02x?}", device.uuid);
                        // Clear stored offset
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
                        // Persist device_offsets to disk via odyssey_hub_common::config
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
                        let mut padded = [0u8; 98];
                        let copy_len = std::cmp::min(data.len(), 98);
                        padded[..copy_len].copy_from_slice(&data[..copy_len]);
                        let vendor = VendorData {
                            len: copy_len as u8,
                            data: padded,
                        };
                        let pkt = Packet {
                            id: 0,
                            data: PacketData::Vendor(tag, vendor),
                        };
                        if let Err(e) = link.lock().await.send(pkt).await {
                            warn!("Failed to send vendor packet: {}", e);
                        }
                    }
                    Some(DeviceTaskMessage::SetShotDelay(ms)) => {
                        debug!(
                            "SetShotDelay requested for {:02x?} = {} ms",
                            device.uuid, ms
                        );
                        // Update in-memory store and notify watchers so handler and others observe change.
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
                        // This shouldn't happen since we only map Some(msg)
                        warn!(
                            "Unexpected None in control message stream for {:02x?}",
                            device.uuid
                        );
                    }
                }
            }
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
async fn process_packet(
    device: &Device,
    pkt: Packet,
    event_sender: &broadcast::Sender<Event>,
    device_offsets: &Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
    fv_state: &mut FoveatedAimpointState,
    screen_calibrations: &Arc<
        ArcSwap<
            ArrayVec<(u8, ScreenCalibration<f32>), { (ats_common::MAX_SCREEN_ID + 1) as usize }>,
        >,
    >,
    prev_accel_timestamp: &mut Option<u32>,
) -> Result<()> {
    // For convenience, keep a clone to forward as raw PacketEvent at the end.
    let raw_pkt = pkt.clone();

    // Map some packet types to high-level events and feed IMU into the provided foveated state.
    match pkt.data {
        PacketData::AccelReport(report) => {
            // Feed IMU into the handler-local foveated state
            {
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
            }

            let ev_kind = DeviceEventKind::AccelerometerEvent(AccelerometerEvent {
                timestamp: report.timestamp,
                accel: report.accel,
                gyro: report.gyro,
                euler_angles: Vector3::zeros(),
            });
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(device.clone(), ev_kind)));
        }
        PacketData::ImpactReport(report) => {
            let ev_kind = DeviceEventKind::ImpactEvent(ImpactEvent {
                timestamp: report.timestamp,
            });
            let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(device.clone(), ev_kind)));
        }
        PacketData::CombinedMarkersReport(report) => {
            // Convert marker points to normalized coordinates (assume 0..4095 range)
            let nf_markers: Vec<ats_cv::foveated::Marker> = report
                .nf_points
                .iter()
                .map(|p| ats_cv::foveated::Marker {
                    position: NaPoint2::new(p.x as f32 / 4095.0, p.y as f32 / 4095.0),
                })
                .collect();
            let wf_markers: Vec<ats_cv::foveated::Marker> = report
                .wf_points
                .iter()
                .map(|p| ats_cv::foveated::Marker {
                    position: NaPoint2::new(p.x as f32 / 4095.0, p.y as f32 / 4095.0),
                })
                .collect();

            // Observe markers in the handler-local foveated state and attempt raycast/raycast_update.
            let mut tracking_pose: Option<Pose> = None;
            let mut aimpoint = Vector2::new(0.0f32, 0.0f32);
            let mut distance = 0.0f32;

            let screen_id = {
                // Use nominal gravity if IMU hasn't set it elsewhere
                let gravity = UnitVector3::new_normalize(Vector3::new(0.0f32, 9.81f32, 0.0f32));

                // Observe markers directly from existing vectors (pass slices, avoid clones)
                let _observed = fv_state.observe_markers(
                    &nf_markers,
                    &wf_markers,
                    gravity,
                    &*screen_calibrations.load(),
                );

                // Attempt to compute pose and aimpoint using raycast_update.
                let sc = screen_calibrations.load();
                let (pose_opt, aim_and_d_opt) = ats_helpers::raycast_update(&*sc, fv_state, None);

                if let Some((rotmat, transvec)) = pose_opt {
                    tracking_pose = Some(Pose {
                        rotation: rotmat,
                        translation: Matrix3x1::new(transvec.x, transvec.y, transvec.z),
                    });
                }

                if let Some((pt, d)) = aim_and_d_opt {
                    aimpoint = pt.coords;
                    distance = d;
                }

                fv_state.screen_id as u32
            };

            // If available, prefer stored device offset pose as additional context (non-blocking).
            if tracking_pose.is_none() {
                let pose_opt = {
                    let guard = device_offsets.lock().await;
                    guard.get(&device.uuid).cloned().map(|iso| Pose {
                        rotation: iso.rotation.to_rotation_matrix().into_inner(),
                        translation: Matrix3x1::new(
                            iso.translation.vector.x,
                            iso.translation.vector.y,
                            iso.translation.vector.z,
                        ),
                    })
                };
                if let Some(p) = pose_opt {
                    tracking_pose = Some(p);
                }
            }

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
        PacketData::PocMarkersReport(report) => {
            // Treat PoC points similarly to CombinedMarkersReport's nearfield
            let nf_markers: Vec<ats_cv::foveated::Marker> = report
                .points
                .iter()
                .map(|p| ats_cv::foveated::Marker {
                    position: NaPoint2::new(p.x as f32 / 4095.0, p.y as f32 / 4095.0),
                })
                .collect();

            let mut tracking_pose: Option<Pose> = None;
            let mut aimpoint = Vector2::new(0.0f32, 0.0f32);
            let mut distance = 0.0f32;

            let screen_id = {
                let gravity = UnitVector3::new_normalize(Vector3::new(0.0f32, 9.81f32, 0.0f32));
                let wf_empty: Vec<ats_cv::foveated::Marker> = Vec::new();
                let _observed = fv_state.observe_markers(
                    &nf_markers,
                    &wf_empty,
                    gravity,
                    &*screen_calibrations.load(),
                );

                let sc = screen_calibrations.load();
                let (pose_opt, aim_and_d_opt) = ats_helpers::raycast_update(&*sc, fv_state, None);

                if let Some((rotmat, transvec)) = pose_opt {
                    tracking_pose = Some(Pose {
                        rotation: rotmat,
                        translation: Matrix3x1::new(transvec.x, transvec.y, transvec.z),
                    });
                }
                if let Some((pt, d)) = aim_and_d_opt {
                    aimpoint = pt.coords;
                    distance = d;
                }
                fv_state.screen_id as u32
            };

            if tracking_pose.is_none() {
                let pose_opt = {
                    let guard = device_offsets.lock().await;
                    guard.get(&device.uuid).cloned().map(|iso| Pose {
                        rotation: iso.rotation.to_rotation_matrix().into_inner(),
                        translation: Matrix3x1::new(
                            iso.translation.vector.x,
                            iso.translation.vector.y,
                            iso.translation.vector.z,
                        ),
                    })
                };
                if let Some(p) = pose_opt {
                    tracking_pose = Some(p);
                }
            }

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
        _ => {
            // other packet types ignored for high-level events here
        }
    }

    // Always emit the raw PacketEvent for consumers that expect raw packets.
    // Construct ats_usb::packets::vm::Packet using the raw data.
    let vm_pkt = ats_usb::packets::vm::Packet {
        id: raw_pkt.id,
        data: raw_pkt.data,
    };
    let _ = event_sender.send(Event::DeviceEvent(DeviceEvent(
        device.clone(),
        DeviceEventKind::PacketEvent(vm_pkt),
    )));

    Ok(())
}
