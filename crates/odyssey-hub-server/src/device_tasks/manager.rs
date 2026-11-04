use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use futures::stream::{self, StreamExt as _};
use nalgebra::Isometry3;
use odyssey_common::ScreenCalibration;
use odyssey_hub_common::device::{Device, DeviceCapabilities};
use odyssey_hub_common::events::Event;
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::PacketData;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, watch};
use tokio::time::sleep;
use tracing::{debug, info, trace, warn};

use super::handler::{DeviceConnections, device_handler_task};
use super::messages::{DeviceChannels, DeviceTaskMessage, DongleInfo, DongleTaskMessage, Message};
use super::netcore_probe::{NetCoreLink, spawn_netcore_probe};
use super::semver::semver_at_least;
use super::usb_probe::{read_uuid_with_retry, uuid_from_usb_serial};
use crate::firmware::{DFU_CLASS, DFU_PROTOCOL_DFU_MODE, DFU_SUBCLASS};

/// Consecutive failed connect attempts to a dongle-reported device before the hub
/// manager waits for a fresh snapshot instead of retrying on its own.
const MAX_HUB_CONNECT_ATTEMPTS: u8 = 5;

/// Events from the hub manager's merged stream
enum HubEvent {
    Snapshot(Result<heapless::Vec<[u8; 6], 3>>),
    /// Timer tick; re-drives snapshot processing for devices that failed to connect.
    Retry,
}

/// Top-level entry for spawning device manager tasks.
///
/// This function spawns the USB device manager which is responsible for discovering
/// USB devices, creating per-device tasks, and managing hub/dongle multiplexing.
pub(crate) async fn device_tasks(
    message_channel: mpsc::Sender<Message>,
    screen_calibrations: Arc<
        ArcSwap<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (odyssey_common::MAX_SCREEN_ID + 1) as usize },
            >,
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
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (odyssey_common::MAX_SCREEN_ID + 1) as usize },
            >,
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
    let device_handles = Arc::new(Mutex::new(HashMap::<
        [u8; 6],
        (mpsc::Sender<DeviceTaskMessage>, tokio::task::JoinHandle<()>),
    >::new()));
    // UUIDs of handlers that were created as direct-USB connections (not BLE-mux primary).
    // These do not need AddControlDevice on each scan pass — their control device is already
    // embedded in the handler. Only UsbMux-primary handlers benefit from AddControlDevice.
    let direct_usb_handler_uuids =
        Arc::new(Mutex::new(std::collections::HashSet::<[u8; 6]>::new()));

    // Hub managers: Vec of (DeviceInfo, slot-with-joinhandle)
    let hub_managers = Arc::new(Mutex::new(Vec::<(
        nusb::DeviceInfo,
        Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    )>::new()));
    let mut direct_usb_id_to_uuid = HashMap::<String, [u8; 6]>::new();
    // The last uuid seen for each product id, which is how a bootloader that
    // reports a placeholder serial can still be matched to the device that just
    // detached. See `migration`.
    let mut last_uuid_by_pid = HashMap::<u16, [u8; 6]>::new();
    // Maps opaque USB device id -> (uuid, pid) for devices detected in DFU mode or with broken
    // protocol.  These are surfaced in the device list so the user can flash firmware even when
    // normal communication is unavailable.
    let mut dfu_device_id_to_info = HashMap::<String, ([u8; 6], u16)>::new();
    // Maps BLE address -> device name, shared between hub managers and direct USB device handlers.
    // Updated on USB rename so BLE reconnects use the correct name.
    let bond_names: Arc<Mutex<HashMap<[u8; 6], String>>> = Arc::new(Mutex::new(HashMap::new()));

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
                        && odyssey_protocol::ProductId::from_u16(d.product_id())
                            == Some(odyssey_protocol::ProductId::Mux)
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
        let mut present_dfu_ids = std::collections::HashSet::<String>::new();
        for dev_info in &devices {
            if dev_info.vendor_id() != 0x1915 {
                continue;
            }

            let pid = dev_info.product_id();

            // Skip non-ATS devices
            let Some(product_id) = odyssey_protocol::ProductId::from_u16(pid) else {
                continue;
            };

            // Note: We no longer skip ATS Lites in BLE mode - we enumerate them for DFU/pairing support
            // The device will be connected using connect_usb_any_mode() which works regardless of transport mode

            if product_id == odyssey_protocol::ProductId::Mux {
                // Hub/dongle - check if we're already managing this hub
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
                        .map(tokio::task::JoinHandle::is_finished)
                        .unwrap_or(true);
                    drop(handle_guard);

                    if task_finished {
                        // Task finished, remove from list so we can reconnect
                        info!(
                            "Hub manager task finished for {:?}, removing from list to allow reconnect",
                            dev_info.id()
                        );
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
                let hub = match odyssey_usb::device::MuxDevice::connect_usb(dev_info.clone()).await
                {
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
                let direct_usb_uuids_clone = direct_usb_handler_uuids.clone();
                let hub_info = dev_info.clone();
                let bond_names_clone = bond_names.clone();
                let hub_manager_handle = tokio::spawn(async move {
                    tracing::info!("Hub manager task started for hub {:?}", hub_info);
                    let bond_names = bond_names_clone;

                    // Track devices managed by this hub for cleanup on exit.
                    // Stores BLE addresses (not UUIDs) so snapshot diffs work correctly
                    // even when a device's UUID differs from its BLE address.
                    let mut hub_devices = std::collections::HashSet::<[u8; 6]>::new();
                    // Maps BLE address -> device UUID for device_handles lookups.
                    let mut hub_device_uuids = HashMap::<[u8; 6], [u8; 6]>::new();

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
                            Ok(bonds) => {
                                tracing::info!(
                                    "list_bonds (startup): {} bonds returned",
                                    bonds.len()
                                );
                                for b in &bonds {
                                    tracing::info!(
                                        "  bond: uuid={:02x?} name={:?}",
                                        b.uuid,
                                        b.name.as_str()
                                    );
                                }
                                let addrs: Vec<[u8; 6]> = bonds.iter().map(|b| b.uuid).collect();
                                {
                                    let mut bn = bond_names.lock().await;
                                    for b in &bonds {
                                        if !b.name.is_empty() {
                                            bn.insert(b.uuid, b.name.to_string());
                                        }
                                    }
                                    tracing::info!(
                                        "bond_names after startup list_bonds: {:?}",
                                        bn.keys().collect::<Vec<_>>()
                                    );
                                }
                                addrs
                            }
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
                                            attempt,
                                            e
                                        );
                                        sleep(Duration::from_millis(200)).await;
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

                    // Periodic tick used to retry devices whose connect failed.
                    // Delay: a blocked connect attempt must not leave a burst of
                    // overdue ticks that would fire retries back to back.
                    let mut retry_interval = tokio::time::interval(Duration::from_secs(2));
                    retry_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    let retry_ticks = stream::unfold(retry_interval, |mut interval| async move {
                        interval.tick().await;
                        Some((HubEvent::Retry, interval))
                    });
                    let combined_stream = stream::select(combined_stream, retry_ticks);
                    tokio::pin!(combined_stream);
                    let mut last_snapshot = heapless::Vec::<[u8; 6], 3>::new();
                    // BLE address -> consecutive failed connect attempts.
                    let mut failed_adds = HashMap::<[u8; 6], u8>::new();

                    loop {
                        tokio::select! {
                            Some(event) = combined_stream.next() => {
                        // A tick only matters while some device failed to connect and
                        // still has attempts left; then re-run the last snapshot so the
                        // device is retried even though the dongle sends no new snapshot.
                        let (event, is_retry) = match event {
                            HubEvent::Retry => {
                                if !failed_adds.values().any(|n| *n < MAX_HUB_CONNECT_ATTEMPTS) {
                                    continue;
                                }
                                (HubEvent::Snapshot(Ok(last_snapshot.clone())), true)
                            }
                            other => (other, false),
                        };
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
                                        last_snapshot = devs.clone();
                                        if !is_retry {
                                            // A fresh snapshot is a fresh chance for every device.
                                            failed_adds.clear();
                                        }
                                        let new_set: HashSet<[u8; 6]> = devs.into_iter().collect();
                                        failed_adds.retain(|a, _| new_set.contains(a));
                                        // Compare against hub_devices (BLE addresses) not device_handles (UUIDs)
                                        let existing_set: HashSet<[u8; 6]> = hub_devices.clone();

                                        tracing::info!("Device diff - new: {:02x?}, existing: {:02x?}, added: {:02x?}, removed: {:02x?}",
                                                        new_set, existing_set,
                                                        new_set.difference(&existing_set).copied().collect::<Vec<_>>(),
                                                        existing_set.difference(&new_set).copied().collect::<Vec<_>>());

                                        // Refresh bond names before processing new devices.
                                        // Use or_insert so we don't overwrite names that were set
                                        // in this session (e.g., via USB rename) with stale dongle flash names.
                                        let added: Vec<[u8; 6]> = new_set.difference(&existing_set).copied().collect();
                                        if !added.is_empty() && supports_list_bonds
                                            && let Ok(bonds) = hub_clone.list_bonds().await {
                                                let mut bn = bond_names.lock().await;
                                                for b in &bonds {
                                                    if !b.name.is_empty() {
                                                        bn.entry(b.uuid).or_insert_with(|| b.name.to_string());
                                                    }
                                                }
                                            }

                                        // Added devices
                                        for addr in &added {
                                            let addr_copy = *addr;
                                            if is_retry
                                                && failed_adds.get(&addr_copy).copied().unwrap_or(0) >= MAX_HUB_CONNECT_ATTEMPTS
                                            {
                                                continue;
                                            }
                                            tracing::info!(
                                                "DevicesSnapshot: Adding new device {:02x?}",
                                                addr_copy
                                            );

                                            // Connect via VmDevice to properly handle streaming
                                            let vm_device = match VmDevice::connect_via_mux(
                                                hub_clone.clone(),
                                                addr_copy,
                                            )
                                            .await
                                            {
                                                Ok(v) => v,
                                                Err(e) => {
                                                    let attempts = failed_adds.entry(addr_copy).or_insert(0);
                                                    *attempts += 1;
                                                    tracing::warn!(
                                                        "Failed to connect to hub device {:02x?} during snapshot (attempt {}/{}): {}",
                                                        addr_copy, attempts, MAX_HUB_CONNECT_ATTEMPTS, e
                                                    );
                                                    continue;
                                                }
                                            };
                                            failed_adds.remove(&addr_copy);

                                            // Read actual UUID from device
                                            let uuid = match vm_device
                                                .read_prop(odyssey_protocol::PropKind::Uuid)
                                                .await
                                            {
                                                Ok(odyssey_protocol::Props::Uuid(uuid_bytes)) => uuid_bytes,
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

                                            // Read firmware and events protocol version via packet interface (works over BLE)
                                            let (version, proto_version) = match vm_device.request(PacketData::ReadVersion()).await {
                                                Ok(PacketData::ReadVersionResponse(v)) => {
                                                    tracing::info!(
                                                        "BLE device {:02x?} firmware version: {}.{}.{}, events protocol: {}.{}.{}",
                                                        uuid,
                                                        v.firmware_semver[0],
                                                        v.firmware_semver[1],
                                                        v.firmware_semver[2],
                                                        v.protocol_semver[0],
                                                        v.protocol_semver[1],
                                                        v.protocol_semver[2],
                                                    );
                                                    ([v.firmware_semver[0], v.firmware_semver[1], v.firmware_semver[2]], v.protocol_semver)
                                                }
                                                Ok(other) => {
                                                    tracing::warn!("Unexpected response reading version from BLE device {:02x?}: {:?}", uuid, other);
                                                    ([0, 0, 0], [0, 0, 0])
                                                }
                                                Err(e) => {
                                                    tracing::warn!("Failed to read version from BLE device {:02x?}: {}", uuid, e);
                                                    ([0, 0, 0], [0, 0, 0])
                                                }
                                            };

                                            let product_id = match vm_device
                                                .read_prop(odyssey_protocol::PropKind::ProductId)
                                                .await
                                            {
                                                Ok(odyssey_protocol::Props::ProductId(id)) => id,
                                                _ => 0,
                                            };

                                            // Read name from device and push to dongle so its bond cache is always fresh.
                                            let name_for_device = match vm_device.read_prop(odyssey_protocol::PropKind::Name).await {
                                                Ok(odyssey_protocol::Props::Name(n)) if !n.is_empty() => {
                                                    let name_str = n.to_string();
                                                    bond_names.lock().await.insert(addr_copy, name_str.clone());
                                                    if let Err(e) = hub_clone.update_bond_name(addr_copy, n.into()).await {
                                                        tracing::warn!("Failed to push name to dongle: {e}");
                                                    }
                                                    Device::name_bytes(&name_str)
                                                }
                                                _ => Device::name_bytes(&bond_names.lock().await.get(&addr_copy).cloned().unwrap_or_default()),
                                            };
                                            let device_meta = Device {
                                                uuid,
                                                transport: odyssey_hub_common::device::Transport::UsbMux,
                                                // EVENTS capability and events_connected will be
                                                // set to true by AddEventsDevice handler once
                                                // streams are actually established.
                                                capabilities: DeviceCapabilities::empty(),
                                                firmware_version: version,
                                                events_transport: odyssey_hub_common::device::EventsTransport::Bluetooth,
                                                events_connected: false,
                                                product_id,
                                                name: name_for_device,
                                                netcore_release: 0,
                                            };

                                            // Check if device is already connected via USB direct before creating handler
                                            let mut dh = device_handles_clone.lock().await;
                                            tracing::info!("BLE mux device {:02x?} discovered (hub path), checking if already connected. device_handles has {} entries", uuid, dh.len());

                                            // Evict stale entries whose handler task has already finished.
                                            // This can happen after SetTransportMode causes the handler to
                                            // break immediately, leaving a dead entry in device_handles.
                                            if let Some((_, existing_handle)) = dh.get(&uuid)
                                                && existing_handle.is_finished() {
                                                    tracing::info!("BLE: Device {:02x?} has a finished handler, evicting stale entry", uuid);
                                                    dh.remove(&uuid);
                                                }

                                            if let Some((existing_cmd_tx, _existing_handle)) = dh.get(&uuid) {
                                                // Device already exists, send AddEventsDevice to merge BLE events
                                                tracing::info!("Device {:02x?} found in device_handles, merging by adding BLE mux for events", uuid);
                                                let result = existing_cmd_tx
                                                    .send(DeviceTaskMessage::AddEventsDevice(vm_device, proto_version))
                                                    .await;
                                                tracing::info!("AddEventsDevice message sent for {:02x?}, result: {:?}", uuid, result);
                                                // Add to hub_devices (by BLE addr) so it gets cleaned up when hub disconnects
                                                hub_devices.insert(addr_copy);
                                                hub_device_uuids.insert(addr_copy, uuid);
                                            } else {
                                                tracing::info!("Device {:02x?} not found in device_handles, creating new BLE mux handler", uuid);
                                                // New device, create handler and add it
                                                drop(dh); // Release lock before spawning

                                                let (ctrl_tx, ctrl_rx) = mpsc::channel::<DeviceTaskMessage>(32);

                                                // Start with no events device; AddEventsDevice is
                                                // queued immediately below so it goes through the
                                                // same handler path as USB-first+BLE (which works).
                                                // This avoids a race where startup reads config
                                                // before streaming, staling the vm_device.
                                                let connections = DeviceConnections {
                                                    control_device: None,
                                                    events_device: None,
                                                    transport_mode: odyssey_protocol::control::device::TransportMode::Ble,
                                                };

                                                let dh_handle = tokio::spawn(device_handler_task(
                                                    connections,
                                                    device_meta.clone(),
                                                    ctrl_rx,
                                                    sc_clone.clone(),
                                                    doff_clone.clone(),
                                                    dsd_clone.clone(),
                                                    swatch_clone.clone(),
                                                    ev_clone.clone(),
                                                    message_channel_clone.clone(),
                                                    direct_usb_uuids_clone.clone(),
                                                    bond_names.clone(),
                                                    proto_version, // events protocol version read via PacketData::ReadVersion()
                                                    Some(hub_clone.clone()),
                                                    ctrl_tx.downgrade(),
                                                ));

                                                let mut dh = device_handles_clone.lock().await;
                                                dh.insert(uuid, (ctrl_tx.clone(), dh_handle));
                                                hub_devices.insert(addr_copy);
                                                hub_device_uuids.insert(addr_copy, uuid);
                                                // Send Connect; name is already set in device_meta
                                                // the device before UpdateDevice can arrive.
                                                let channels = DeviceChannels {
                                                    commands: Some(ctrl_tx.clone()),
                                                };
                                                let _ = message_channel_clone
                                                    .send(Message::Connect(device_meta, channels))
                                                    .await;

                                                // Queue AddEventsDevice so streams are started
                                                // via the handler (start_sensor_streams before
                                                // read_all_config, consistent with USB-first path).
                                                let _ = ctrl_tx
                                                    .send(DeviceTaskMessage::AddEventsDevice(vm_device, proto_version))
                                                    .await;
                                            }
                                        }

                                        // Removed devices - only remove devices that were managed by THIS hub
                                        let to_remove: Vec<[u8; 6]> = hub_devices
                                            .iter()
                                            .copied()
                                            .filter(|a| !new_set.contains(a))
                                            .collect();
                                        for addr in to_remove {
                                            let uuid = hub_device_uuids.remove(&addr).unwrap_or(addr);
                                            tracing::info!(
                                                        "DevicesSnapshot: BLE device {:02x?} (UUID {:02x?}) disconnected from hub",
                                                        addr, uuid
                                                    );

                                            // Remove from hub_devices tracking
                                            hub_devices.remove(&addr);

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
                                                Ok(bonds) => {
                                                    let addrs: Vec<[u8; 6]> = bonds.iter().map(|b| b.uuid).collect();
                                                    {
                                                        let mut bn = bond_names.lock().await;
                                                        for b in &bonds {
                                                            if !b.name.is_empty() {
                                                                // Use or_insert_with so in-session renames
                                                                // (which already updated bond_names) are not
                                                                // overwritten by stale dongle flash names.
                                                                bn.entry(b.uuid).or_insert_with(|| b.name.to_string());
                                                            }
                                                        }
                                                    }
                                                    addrs
                                                }
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
                            HubEvent::Retry => unreachable!("converted to a snapshot above"),
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
                                            let bond_names_clone = bond_names.clone();
                                            tokio::spawn(async move {
                                                let (success, paired_address, paired_name, error) = match hub_wait.wait_pairing_event().await {
                                                    Ok(odyssey_usb::device::PairingEvent::Result(device)) => (true, device.uuid, device.name.to_string(), String::new()),
                                                    Ok(odyssey_usb::device::PairingEvent::Timeout) => (false, [0; 6], String::new(), "Pairing timed out".into()),
                                                    Ok(odyssey_usb::device::PairingEvent::Cancelled) => (false, [0; 6], String::new(), "Pairing cancelled".into()),
                                                    Err(e) => (false, [0; 6], String::new(), e.to_string()),
                                                };
                                                if success && !paired_name.is_empty() {
                                                    tracing::info!("pairing: storing name {:?} for addr={:02x?}", paired_name, paired_address);
                                                    bond_names_clone.lock().await.insert(paired_address, paired_name.clone());
                                                }
                                                let _ = ev_tx.send(Event::DonglePairingResult {
                                                    dongle_id: did,
                                                    success,
                                                    paired_address,
                                                    paired_name,
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
                    for addr in hub_devices {
                        let uuid = hub_device_uuids.get(&addr).copied().unwrap_or(addr);
                        let dh = device_handles_clone.lock().await;
                        if let Some((cmd_tx, _handle)) = dh.get(&uuid) {
                            // Send RemoveEventsDevice to remove BLE mux events
                            // The handler will decide if it should exit (no control) or continue (has USB control)
                            tracing::info!(
                                "Sending RemoveEventsDevice for addr {:02x?} (UUID {:02x?})",
                                addr,
                                uuid
                            );
                            let _ = cmd_tx.send(DeviceTaskMessage::RemoveEventsDevice).await;
                        } else {
                            tracing::warn!(
                                "Device addr {:02x?} (UUID {:02x?}) not found in device_handles during cleanup",
                                addr,
                                uuid
                            );
                        }
                    }
                    tracing::info!("Hub manager task ended for hub {:?}", hub_info);
                });

                // Store hub manager handle for tracking (runs indefinitely until hub is disconnected)
                {
                    let mut hm = hub_managers.lock().await;
                    // Wrap in Arc<Mutex<Option<_>>> so the handle can be taken (consumed) later
                    // while still being held behind a shared reference.
                    let hub_slot = Arc::new(Mutex::new(Some(hub_manager_handle)));
                    hm.push((dev_info.clone(), hub_slot));
                }
            } else {
                // Direct USB device (VmDevice) - AtsVm, AtsLite, or AtsPro
                // First check if we should even connect to this device
                let direct_usb_id = format!("{:?}", dev_info.id());
                present_direct_usb_ids.insert(direct_usb_id.clone());
                debug!(
                    "Found direct USB device {:?}, checking if already connected",
                    product_id
                );

                // Connect and read UUID/version/transport mode first
                // We'll keep this connection if we decide to use it
                let (uuid, version, events_proto_semver, transport_mode, vm_device_temp) = {
                    match VmDevice::connect_usb_any_mode(dev_info.clone()).await {
                        Ok(vm_device) => {
                            // Read UUID - use control plane for AtsLite/AtsPro (control endpoint),
                            // packet protocol for AtsVm. Retries up to 3 times.
                            let Some(uuid) = read_uuid_with_retry(&vm_device, product_id).await
                            else {
                                continue;
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
                                    odyssey_protocol::control::device::TransportMode::Usb
                                }
                            };

                            // Read events protocol version via packet interface.
                            // Only available when device is in USB transport mode (direct events).
                            let events_proto_semver = if transport_mode
                                == odyssey_protocol::control::device::TransportMode::Usb
                            {
                                match vm_device
                                    .request_with_retry(
                                        PacketData::ReadVersion(),
                                        Duration::from_secs(2),
                                        3,
                                    )
                                    .await
                                {
                                    Ok(PacketData::ReadVersionResponse(v)) => {
                                        info!(
                                            "Device {:02x?} events protocol version: {}.{}.{}",
                                            uuid,
                                            v.protocol_semver[0],
                                            v.protocol_semver[1],
                                            v.protocol_semver[2]
                                        );
                                        v.protocol_semver
                                    }
                                    Ok(other) => {
                                        warn!(
                                            "Unexpected response reading events version from {:02x?}: {:?}",
                                            uuid, other
                                        );
                                        [0, 0, 0]
                                    }
                                    Err(e) => {
                                        warn!(
                                            "Failed to read events protocol version from {:02x?}: {}",
                                            uuid, e
                                        );
                                        [0, 0, 0]
                                    }
                                }
                            } else {
                                [0, 0, 0]
                            };

                            (
                                uuid,
                                version,
                                events_proto_semver,
                                transport_mode,
                                vm_device,
                            )
                        }
                        Err(e) => {
                            debug!("Failed to connect to USB device: {}", e);
                            // If we already have a running handler for this USB device (interface
                            // is claimed), the connect failure is expected — skip DFU surfacing.
                            if let Some(&existing_uuid) = direct_usb_id_to_uuid.get(&direct_usb_id)
                                && device_handles.lock().await.contains_key(&existing_uuid)
                            {
                                continue;
                            }
                            // Connection failed — surface the device for firmware update if it
                            // really is sitting in the bootloader. An application-mode ATS Lite
                            // also exposes a class 0xFE interface, for the DFU *runtime*
                            // descriptor it uses to accept a detach, so the class alone says
                            // nothing: only protocol 0x02 means the bootloader is running. The
                            // serial number encodes the UUID in both modes.
                            let in_dfu_mode = dev_info.interfaces().any(|i| {
                                i.class() == DFU_CLASS
                                    && i.subclass() == DFU_SUBCLASS
                                    && i.protocol() == DFU_PROTOCOL_DFU_MODE
                            });
                            if in_dfu_mode {
                                let uuid = match uuid_from_usb_serial(dev_info.serial_number()) {
                                    Some(uuid) => Some(uuid),
                                    // Bootloaders that predate per-device serials cannot
                                    // identify themselves. Prefer the device last seen
                                    // under this product id; a device that has been in
                                    // the bootloader since before the app started has
                                    // never been seen, so it gets a stand-in instead,
                                    // which is the only way it reaches the device list
                                    // and therefore the only way it can be recovered.
                                    None if crate::migration::is_placeholder_serial(
                                        dev_info.serial_number(),
                                    ) =>
                                    {
                                        last_uuid_by_pid.get(&pid).copied().or_else(|| {
                                            crate::migration::recovery_uuid(
                                                dev_info.vendor_id(),
                                                pid,
                                            )
                                        })
                                    }
                                    None => None,
                                };
                                let Some(uuid) = uuid else {
                                    warn!(
                                        "Device PID 0x{:04x} is in DFU mode but reports no usable serial and was not seen before — cannot surface for update",
                                        pid
                                    );
                                    continue;
                                };
                                present_dfu_ids.insert(direct_usb_id.clone());
                                // The rest of the system knows a device by the
                                // product id its application enumerates under,
                                // which is not the one the bootloader uses.
                                let app_pid = crate::firmware::app_pid(pid);
                                let already_tracked =
                                    device_handles.lock().await.contains_key(&uuid)
                                        || dfu_device_id_to_info.values().any(|(u, _)| u == &uuid);
                                if !already_tracked {
                                    let bootloader = crate::firmware::bootloader_release(
                                        dev_info.device_version(),
                                    );
                                    info!(
                                        "Device {:02x?} is in DFU mode (PID 0x{:04x}, bootloader {:?}), surfacing for firmware update (connect error: {})",
                                        uuid, pid, bootloader, e
                                    );
                                    dfu_device_id_to_info
                                        .insert(direct_usb_id.clone(), (uuid, app_pid));
                                    let dfu_meta = Device {
                                        uuid,
                                        transport: odyssey_hub_common::device::Transport::Usb,
                                        capabilities: DeviceCapabilities::new(
                                            DeviceCapabilities::DFU,
                                        ),
                                        // No application is running, so there is no
                                        // firmware version to report.
                                        firmware_version: [0, 0, 0],
                                        events_transport:
                                            odyssey_hub_common::device::EventsTransport::Wired,
                                        events_connected: false,
                                        product_id: app_pid,
                                        name: Device::name_bytes(""),
                                        netcore_release: 0,
                                    };
                                    let _ = message_channel
                                        .send(Message::Connect(
                                            dfu_meta,
                                            DeviceChannels { commands: None },
                                        ))
                                        .await;
                                } else {
                                    // Already tracked — just keep it alive in present set
                                    if dfu_device_id_to_info.contains_key(&direct_usb_id) {
                                        present_dfu_ids.insert(direct_usb_id.clone());
                                    }
                                }
                            }
                            continue;
                        }
                    }
                    // Keep vm_device alive - we'll either use it or drop it based on should_connect
                };

                // Cache mapping from opaque nusb id() to UUID once we've successfully probed.
                direct_usb_id_to_uuid.insert(direct_usb_id, uuid);
                last_uuid_by_pid.insert(pid, uuid);

                // Now check if device is already connected BEFORE creating a new connection
                let should_connect = {
                    let mut dh = device_handles.lock().await;
                    info!(
                        "Checking if device {:02x?} is already connected (current handles: {})",
                        uuid,
                        dh.len()
                    );

                    // Evict stale entries whose handler task has already finished.
                    // This can happen after SetTransportMode causes the handler to break
                    // immediately, leaving a dead entry in device_handles. Without this
                    // eviction the next scan pass would skip re-connecting the device.
                    if let Some((_, existing_handle)) = dh.get(&uuid)
                        && existing_handle.is_finished()
                    {
                        info!(
                            "Device {:02x?} has a finished handler, evicting stale entry",
                            uuid
                        );
                        dh.remove(&uuid);
                        direct_usb_handler_uuids.lock().await.remove(&uuid);
                    }

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
                            spawn_netcore_probe(
                                NetCoreLink::Control(vm_device_temp.clone()),
                                version.as_ref().map(|v| v.protocol_semver),
                                existing_cmd_tx.clone(),
                            );
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
                // DFU capability is present whenever the device exposes a DFU interface (class 0xFE)
                let has_dfu_iface = dev_info.interfaces().any(|i| i.class() == 0xFE);
                let mut capabilities = DeviceCapabilities::new(DeviceCapabilities::CONTROL);
                if transport_mode == odyssey_protocol::control::device::TransportMode::Usb {
                    capabilities.insert(DeviceCapabilities::EVENTS);
                }
                if has_dfu_iface {
                    capabilities.insert(DeviceCapabilities::DFU);
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
                        == odyssey_protocol::control::device::TransportMode::Usb
                    {
                        odyssey_hub_common::device::EventsTransport::Wired
                    } else {
                        odyssey_hub_common::device::EventsTransport::Bluetooth
                    },
                    events_connected: transport_mode
                        == odyssey_protocol::control::device::TransportMode::Usb,
                    product_id: pid,
                    name: Device::name_bytes(
                        &dev_info
                            .product_string()
                            .filter(|s| !s.is_empty())
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                    ),
                    netcore_release: 0,
                };

                let (tx, rx) = mpsc::channel(32);

                // USB direct devices: always have control, events only if in USB mode
                let connections =
                    if transport_mode == odyssey_protocol::control::device::TransportMode::Usb {
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

                let netcore_control = connections.control_device.clone();
                let initial_protocol_version = events_proto_semver;
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
                    direct_usb_handler_uuids.clone(),
                    bond_names.clone(),
                    initial_protocol_version,
                    None, // USB-direct device: no hub to update
                    tx.downgrade(),
                ));

                {
                    let mut dh = device_handles.lock().await;
                    dh.insert(uuid, (tx.clone(), handle));
                    direct_usb_handler_uuids.lock().await.insert(uuid);
                }
                let netcore_probe = (
                    netcore_control,
                    version.as_ref().map(|v| v.protocol_semver),
                    tx.clone(),
                );

                // Always pass the commands channel so server.device_list gets a working sender.
                // The device handler task will process commands (like Zero) once events are available.
                let channels = DeviceChannels { commands: Some(tx) };

                let _ = message_channel
                    .send(Message::Connect(device_meta.clone(), channels))
                    .await;

                if let (Some(control), protocol, tx) = netcore_probe {
                    spawn_netcore_probe(NetCoreLink::Control(control), protocol, tx);
                }
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
                    // A unit that re-enumerates (a reboot, a transport switch) shows up
                    // at a new address while its old one is still in the map. The
                    // handler for that UUID is then the new connection's, so telling it
                    // its USB control left would tear it down seconds after it started.
                    let reappeared = direct_usb_id_to_uuid.iter().any(|(other, other_uuid)| {
                        *other_uuid == uuid && present_direct_usb_ids.contains(other)
                    });
                    if reappeared {
                        tracing::info!(
                            "Direct USB address {} of {:02x?} is gone but the unit is present at another address",
                            id,
                            uuid
                        );
                        continue;
                    }
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

        // Reconcile DFU device disconnects.
        let missing_dfu_ids: Vec<String> = dfu_device_id_to_info
            .keys()
            .filter(|id| !present_dfu_ids.contains(*id))
            .cloned()
            .collect();
        for id in missing_dfu_ids {
            if let Some((uuid, pid)) = dfu_device_id_to_info.remove(&id) {
                tracing::info!(
                    "DFU device missing from scan, disconnecting: id={} uuid={:02x?}",
                    id,
                    uuid
                );
                let disconnect_device = Device {
                    uuid,
                    transport: odyssey_hub_common::device::Transport::Usb,
                    capabilities: DeviceCapabilities::new(DeviceCapabilities::DFU),
                    firmware_version: [0, 0, 0],
                    events_transport: odyssey_hub_common::device::EventsTransport::Wired,
                    events_connected: false,
                    product_id: pid,
                    name: Device::name_bytes(""),
                    netcore_release: 0,
                };
                let _ = message_channel
                    .send(Message::Disconnect(disconnect_device))
                    .await;
            }
        }

        sleep(Duration::from_secs(2)).await;
    }
}
