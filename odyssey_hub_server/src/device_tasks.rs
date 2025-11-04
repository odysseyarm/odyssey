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

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch, Mutex, RwLock};
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::device_link::DeviceLink;
use crate::usb_link::UsbLink;
use ats_usb::packets::hub::HubMsg;

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
    device_offsets: Arc<Mutex<HashMap<u64, Isometry3<f32>>>>,
    device_shot_delays: Arc<RwLock<HashMap<u64, u16>>>,
    shot_delay_watch: Arc<RwLock<HashMap<u64, watch::Sender<u32>>>>,
    _accessory_map: Arc<
        std::sync::Mutex<HashMap<[u8; 6], (odyssey_hub_common::accessory::AccessoryInfo, bool)>>,
    >,
    _accessory_map_sender: broadcast::Sender<odyssey_hub_common::accessory::AccessoryMap>,
    _accessory_info_receiver: watch::Receiver<odyssey_hub_common::accessory::AccessoryInfoMap>,
    event_sender: broadcast::Sender<Event>,
) -> Result<()> {
    tokio::select! {
        _ = usb_device_manager(
            message_channel.clone(),
            screen_calibrations.clone(),
            device_offsets.clone(),
            device_shot_delays.clone(),
            shot_delay_watch.clone(),
            event_sender.clone(),
        ) => {},
    }
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
    device_offsets: Arc<Mutex<HashMap<u64, Isometry3<f32>>>>,
    device_shot_delays: Arc<RwLock<HashMap<u64, u16>>>,
    shot_delay_watch: Arc<RwLock<HashMap<u64, watch::Sender<u32>>>>,
    event_sender: broadcast::Sender<Event>,
) {
    // Map of uuid -> JoinHandle for per-device tasks
    let device_handles = Arc::new(tokio::sync::Mutex::new(HashMap::<
        u64,
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
                    // Hub/dongle
                    match ats_usb::device::HubDevice::connect_usb(dev_info.clone()).await {
                        Ok(hub) => {
                            info!("Connected USB hub device: {:?}", dev_info);

                            // Per-hub map: BLE addr -> sender of Packet
                            let per_hub_senders = Arc::new(tokio::sync::Mutex::new(HashMap::<
                                [u8; 6],
                                mpsc::Sender<Packet>,
                            >::new(
                            )));

                            // Create device handlers for current hub snapshot
                            match hub.request_devices().await {
                                Ok(list) => {
                                    for addr in list.iter() {
                                        let addr_copy = *addr;
                                        let (pkt_tx, pkt_rx) = mpsc::channel::<Packet>(128);
                                        let link = crate::usb_hub_link::UsbHubLink::from_connected_hub_with_rx(
                                            hub.clone(),
                                            addr_copy,
                                            pkt_rx,
                                        );
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

                                        let mut uuid_bytes = [0u8; 8];
                                        uuid_bytes[..6].copy_from_slice(&addr_copy);
                                        let uuid = u64::from_le_bytes(uuid_bytes);

                                        {
                                            let mut dh = device_handles.lock().await;
                                            if !dh.contains_key(&uuid) {
                                                dh.insert(uuid, dh_handle);
                                            }
                                        }

                                        {
                                            let mut map = per_hub_senders.lock().await;
                                            map.insert(addr_copy, pkt_tx.clone());
                                        }

                                        let device_meta = Device {
                                            uuid,
                                            transport: odyssey_hub_common::device::Transport::BLE,
                                        };
                                        let _ = message_channel
                                            .send(Message::Connect(device_meta.clone(), ctrl_tx))
                                            .await;
                                    }
                                }
                                Err(e) => {
                                    debug!("Failed to request device list from hub: {}", e);
                                }
                            }

                            // Hub manager task: route HubMsg -> per-device channels and handle snapshots
                            let hub_clone = hub.clone();
                            let per_hub_senders_clone = per_hub_senders.clone();
                            let device_handles_clone = device_handles.clone();
                            let hub_managers_clone = hub_managers.clone();
                            let message_channel_clone = message_channel.clone();
                            let sc_clone = screen_calibrations.clone();
                            let doff_clone = device_offsets.clone();
                            let dsd_clone = device_shot_delays.clone();
                            let swatch_clone = shot_delay_watch.clone();
                            let ev_clone = event_sender.clone();
                            let hub_info = dev_info.clone();

                            let hub_manager_handle = tokio::spawn(async move {
                                loop {
                                    match hub_clone.receive_msg().await {
                                        Ok(msg) => match msg {
                                            HubMsg::DevicePacket(dev_pkt) => {
                                                let tx_opt = {
                                                    let map = per_hub_senders_clone.lock().await;
                                                    map.get(&dev_pkt.dev).cloned()
                                                };
                                                if let Some(tx) = tx_opt {
                                                    if tx.send(dev_pkt.pkt).await.is_err() {
                                                        let mut map =
                                                            per_hub_senders_clone.lock().await;
                                                        map.remove(&dev_pkt.dev);
                                                    }
                                                }
                                            }
                                            HubMsg::DevicesSnapshot(devs) => {
                                                use std::collections::HashSet;
                                                let new_set: HashSet<[u8; 6]> =
                                                    devs.into_iter().collect();
                                                let existing_set: HashSet<[u8; 6]> = {
                                                    let map = per_hub_senders_clone.lock().await;
                                                    map.keys().cloned().collect()
                                                };

                                                // Added devices
                                                for addr in new_set.difference(&existing_set) {
                                                    let addr_copy = *addr;
                                                    let (pkt_tx, pkt_rx) =
                                                        mpsc::channel::<Packet>(128);
                                                    let link = crate::usb_hub_link::UsbHubLink::from_connected_hub_with_rx(
                                                        hub_clone.clone(),
                                                        addr_copy,
                                                        pkt_rx,
                                                    );
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

                                                    let mut uuid_bytes = [0u8; 8];
                                                    uuid_bytes[..6].copy_from_slice(&addr_copy);
                                                    let uuid = u64::from_le_bytes(uuid_bytes);

                                                    {
                                                        let mut dh =
                                                            device_handles_clone.lock().await;
                                                        if !dh.contains_key(&uuid) {
                                                            dh.insert(uuid, dh_handle);
                                                        }
                                                    }

                                                    {
                                                        let mut map =
                                                            per_hub_senders_clone.lock().await;
                                                        map.insert(addr_copy, pkt_tx);
                                                    }

                                                    let device_meta = Device {
                                                        uuid,
                                                        transport: odyssey_hub_common::device::Transport::BLE,
                                                    };
                                                    let _ = message_channel_clone
                                                        .send(Message::Connect(
                                                            device_meta.clone(),
                                                            ctrl_tx,
                                                        ))
                                                        .await;
                                                }

                                                // Removed devices
                                                let to_remove: Vec<[u8; 6]> = {
                                                    let map = per_hub_senders_clone.lock().await;
                                                    map.keys()
                                                        .cloned()
                                                        .filter(|a| !new_set.contains(a))
                                                        .collect()
                                                };
                                                for addr in to_remove {
                                                    {
                                                        let mut map =
                                                            per_hub_senders_clone.lock().await;
                                                        map.remove(&addr);
                                                    }
                                                    let mut uuid_bytes = [0u8; 8];
                                                    uuid_bytes[..6].copy_from_slice(&addr);
                                                    let uuid = u64::from_le_bytes(uuid_bytes);
                                                    if let Some(handle) = device_handles_clone
                                                        .lock()
                                                        .await
                                                        .remove(&uuid)
                                                    {
                                                        handle.abort();
                                                    }
                                                    let device_meta = Device {
                                                        uuid,
                                                        transport: odyssey_hub_common::device::Transport::BLE,
                                                    };
                                                    let _ = message_channel_clone
                                                        .send(Message::Disconnect(device_meta))
                                                        .await;
                                                }
                                            }
                                            _ => {}
                                        },
                                        Err(e) => {
                                            tracing::warn!(
                                                "HubDevice receive error in hub manager: {}",
                                                e
                                            );
                                            break;
                                        }
                                    }
                                }
                                tracing::info!("Hub manager task ended for hub {:?}", hub_info);
                            });

                            // Store hub manager slot for later abort/observe
                            let hub_slot = std::sync::Arc::new(tokio::sync::Mutex::new(Some(
                                hub_manager_handle,
                            )));
                            {
                                let mut hm = hub_managers.lock().await;
                                hm.push((dev_info.clone(), hub_slot.clone()));
                            }

                            // Observer to remove slot when manager completes
                            let hub_managers_clone2 = hub_managers.clone();
                            let hub_id_for_watch = dev_info.clone();
                            tokio::spawn(async move {
                                let handle_opt = {
                                    let mut slot = hub_slot.lock().await;
                                    slot.take()
                                };
                                if let Some(handle) = handle_opt {
                                    match tokio::time::timeout(Duration::from_secs(5), handle).await
                                    {
                                        Ok(res) => tracing::info!(
                                            "Observed hub manager completion for hub {:?}: {:?}",
                                            hub_id_for_watch,
                                            res
                                        ),
                                        Err(_) => tracing::info!(
                                            "Hub manager for {:?} did not exit within grace period",
                                            hub_id_for_watch
                                        ),
                                    }
                                }
                                let mut hm = hub_managers_clone2.lock().await;
                                if let Some(pos) = hm
                                    .iter()
                                    .position(|(id, _)| id.id() == hub_id_for_watch.id())
                                {
                                    hm.remove(pos);
                                }
                            });
                        }
                        Err(e) => {
                            debug!("Failed to connect to USB hub device: {}", e);
                        }
                    }
                } else {
                    // Direct USB device (VmDevice)
                    match UsbLink::connect_usb(dev_info.clone()).await {
                        Ok(link) => {
                            let device = link.device();
                            let uuid = device.uuid;

                            {
                                let mut dh = device_handles.lock().await;
                                if dh.contains_key(&uuid) {
                                    continue;
                                }

                                info!("Discovered USB device with UUID {:016x}", uuid);

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
                    info!("Device {:016x} task finished", uuid);
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
    device_offsets: Arc<Mutex<HashMap<u64, Isometry3<f32>>>>,
    device_shot_delays: Arc<RwLock<HashMap<u64, u16>>>,
    shot_delay_watch: Arc<RwLock<HashMap<u64, watch::Sender<u32>>>>,
    event_sender: broadcast::Sender<Event>,
) {
    let device = link.device();
    info!(
        "Starting device handler for device UUID {:016x}",
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
        "Initial shot delay for {:016x} = {} ms",
        device.uuid, current_shot_delay
    );
    // Per-device foveated state (local to this handler) and previous IMU timestamp.
    // Keeping these local avoids a global registry and ties filter lifetime to the device task.
    let mut fv_state = FoveatedAimpointState::new();
    let mut prev_accel_timestamp: Option<u32> = None;

    loop {
        tokio::select! {
            pkt_result = link.recv() => {
                match pkt_result {
                    Ok(pkt) => {
                        if let Err(e) = process_packet(&device, pkt.clone(), &event_sender, &device_offsets, &mut fv_state, &screen_calibrations, &mut prev_accel_timestamp).await {
                            warn!("Error processing packet: {}", e);
                        }
                    }
                    Err(e) => {
                        warn!("Device recv error: {}", e);
                        break;
                    }
                }
            }

            changed = shot_delay_rx.changed() => {
                match changed {
                    Ok(_) => {
                        current_shot_delay = *shot_delay_rx.borrow_and_update();
                        info!("Shot delay updated for {:016x} = {} ms", device.uuid, current_shot_delay);
                        // Update in-memory store as well (keeps server and task in sync)
                        device_shot_delays.write().await.insert(device.uuid, current_shot_delay as u16);
                        // Do not emit a ShotDelayChanged DeviceEvent here — the server RPCs
                        // and other code paths are responsible for broadcasting higher-level updates.
                    }
                    Err(_) => {
                        warn!("Shot delay watch sender dropped for {:016x}", device.uuid);
                    }
                }
            }

            msg = control_rx.recv() => {
                match msg {
                    Some(DeviceTaskMessage::ResetZero) => {
                        debug!("ResetZero requested for {:016x}", device.uuid);
                        // Clear stored offset
                        {
                            let mut guard = device_offsets.lock().await;
                            guard.remove(&device.uuid);
                        }
                        let ev = Event::DeviceEvent(DeviceEvent(device.clone(), DeviceEventKind::ZeroResult(true)));
                        let _ = event_sender.send(ev);
                    }
                    Some(DeviceTaskMessage::SaveZero) => {
                        debug!("SaveZero requested for {:016x}", device.uuid);
                        // Persist device_offsets to disk via odyssey_hub_common::config
                        {
                            let guard = device_offsets.lock().await;
                            if let Err(e) = odyssey_hub_common::config::device_offsets_save_async(&*guard).await {
                                tracing::warn!("Failed to save zero offsets: {}", e);
                            }
                        }
                        let ev = Event::DeviceEvent(DeviceEvent(device.clone(), DeviceEventKind::SaveZeroResult(true)));
                        let _ = event_sender.send(ev);
                    }
                    Some(DeviceTaskMessage::WriteVendor(tag, data)) => {
                        debug!("WriteVendor tag={} len={} for {:016x}", tag, data.len(), device.uuid);
                        let mut padded = [0u8; 98];
                        let copy_len = std::cmp::min(data.len(), 98);
                        padded[..copy_len].copy_from_slice(&data[..copy_len]);
                        let vendor = VendorData { len: copy_len as u8, data: padded };
                        let pkt = Packet { id: 0, data: PacketData::Vendor(tag, vendor) };
                        if let Err(e) = link.send(pkt).await {
                            warn!("Failed to send vendor packet: {}", e);
                        }
                    }
                    Some(DeviceTaskMessage::SetShotDelay(ms)) => {
                        debug!("SetShotDelay requested for {:016x} = {} ms", device.uuid, ms);
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
                        debug!("Zero received - storing device offset for {:016x}", device.uuid);
                        let iso = Isometry3::from_parts(Translation3::from(trans.vector), UnitQuaternion::identity());
                        {
                            let mut guard = device_offsets.lock().await;
                            guard.insert(device.uuid, iso);
                        }
                        let ev = Event::DeviceEvent(DeviceEvent(device.clone(), DeviceEventKind::ZeroResult(true)));
                        let _ = event_sender.send(ev);
                    }
                    Some(DeviceTaskMessage::ClearZero) => {
                        debug!("ClearZero - removing stored device offset for {:016x}", device.uuid);
                        {
                            let mut guard = device_offsets.lock().await;
                            guard.remove(&device.uuid);
                        }
                        let ev = Event::DeviceEvent(DeviceEvent(device.clone(), DeviceEventKind::SaveZeroResult(true)));
                        let _ = event_sender.send(ev);
                    }
                    None => {
                        info!("Control channel closed for {:016x}", device.uuid);
                        break;
                    }
                }
            }
        }
    }

    info!("Device handler task ending for {:016x}", device.uuid);
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
    device_offsets: &Arc<Mutex<HashMap<u64, Isometry3<f32>>>>,
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
            let mut screen_id = 0u32;

            {
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

                screen_id = fv_state.screen_id as u32;
            }

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
            let mut screen_id = 0u32;

            {
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
                screen_id = fv_state.screen_id as u32;
            }

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
