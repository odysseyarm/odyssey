use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use futures::stream::{BoxStream, SelectAll, StreamExt as _};
use nalgebra::{Isometry3, Rotation3};
use odyssey_common::ScreenCalibration;
use odyssey_cv::foveated::FoveatedAimpointState;
use odyssey_cv::helpers as ats_helpers;
use odyssey_hub_common::device::{Device, DeviceCapabilities};
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event};
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::{ConfigKind, GeneralConfig, PacketData};
use opencv_ros_camera::RosOpenCvIntrinsics;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, watch};
use tokio::time::sleep;
use tracing::{debug, info, warn};

use super::messages::{DeviceTaskMessage, Message};
use super::netcore_probe::spawn_packet_netcore_probe;
use super::sensor_streams::{DeviceStreamEvent, start_sensor_streams};
use super::tracking::{calculate_zero_offset_from_tracking_event, handle_stream_event};

/// Device connections - can have separate VmDevice instances for control vs events
pub(super) struct DeviceConnections {
    /// VmDevice for control operations (USB direct connection)
    pub(super) control_device: Option<VmDevice>,
    /// VmDevice for event streams (USB direct or BLE mux)
    pub(super) events_device: Option<VmDevice>,
    /// Transport mode of the primary device
    pub(super) transport_mode: odyssey_protocol::control::device::TransportMode,
}

/// Per-device handler: receives packets from the link, processes control messages,
/// watches for shot-delay changes, and emits events.
pub(super) async fn device_handler_task(
    mut connections: DeviceConnections,
    mut device: Device,
    control_rx: mpsc::Receiver<DeviceTaskMessage>,
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
    message_channel: mpsc::Sender<Message>,
    direct_usb_handler_uuids: Arc<Mutex<std::collections::HashSet<[u8; 6]>>>,
    bond_names: Arc<Mutex<HashMap<[u8; 6], String>>>,
    mut protocol_version: [u16; 3],
    hub_device: Option<odyssey_usb::device::MuxDevice>,
    self_tx: mpsc::WeakSender<DeviceTaskMessage>,
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
            let cur = u32::from(
                device_shot_delays
                    .read()
                    .await
                    .get(&device.uuid)
                    .copied()
                    .unwrap_or(0),
            );
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

    // All config (camera models, impact threshold, etc.) is readable over both
    // USB and BLE via packet protocol. Prefer the events device; fall back to
    // the control device only when no events device is present.
    let general_settings = if let Some(events_device) = connections.events_device.as_ref() {
        info!(
            "Reading config for device {:02x?} from events device",
            device.uuid
        );
        match tokio::time::timeout(Duration::from_secs(5), events_device.read_all_config()).await {
            Ok(Ok(cfg)) => {
                info!("Successfully read config for device {:02x?}", device.uuid);
                Some(cfg)
            }
            Ok(Err(e)) => {
                warn!(
                    "Failed to read config for device {:02x?}: {}",
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
    } else if let Some(ctrl_device) = connections.control_device.as_ref() {
        info!(
            "Reading config for device {:02x?} via control plane (no events device)",
            device.uuid
        );
        match tokio::time::timeout(Duration::from_secs(10), ctrl_device.read_all_config_ctrl())
            .await
        {
            Ok(Ok(cfg)) => {
                info!(
                    "Successfully read config for device {:02x?} via control plane",
                    device.uuid
                );
                Some(cfg)
            }
            Ok(Err(e)) => {
                warn!(
                    "Failed to read config for device {:02x?} via control plane: {}",
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
            "Skipping config for device {:02x?} (no events or control device)",
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
                cfg.stereo_iso,
            )
        } else {
            // Device in BLE mode - use dummy camera models
            // These will be updated when AddEventsDevice is called
            info!(
                "Device {:02x?} starting in BLE mode with dummy camera models",
                device.uuid
            );
            (
                RosOpenCvIntrinsics::from_params(1.0, 0.0, 1.0, 0.0, 0.0),
                RosOpenCvIntrinsics::from_params(1.0, 0.0, 1.0, 0.0, 0.0),
                Isometry3::identity(),
            )
        };

    info!("Starting sensor streams for device {:02x?}", device.uuid);
    // Start sensor streams via VmDevice helpers so each stream reserves a slot
    // Only start streams if we have an events device (USB mode or BLE mux)
    let mut sensor_streams: SelectAll<BoxStream<'static, DeviceStreamEvent>> = SelectAll::new();

    if let Some(events_device) = connections.events_device.as_ref() {
        sensor_streams = start_sensor_streams(events_device, device.uuid, protocol_version).await;
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

    // Debounce state for register reads/writes: pause streams once before the first read
    // in a sequence and restart them 100 ms after the last read
    // TODO this is kind of overengineered
    let mut streams_paused = false;
    let streams_resume_sleep = sleep(Duration::from_secs(3600));
    tokio::pin!(streams_resume_sleep);
    let mut streams_resume_armed = false;

    // Retry sensor streams every 5 seconds when they are not running (e.g. broken protocol).
    let streams_retry_sleep = sleep(Duration::from_secs(5));
    tokio::pin!(streams_retry_sleep);

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
                info!("control_stream received '{:?}' for {:02x?}", maybe_msg, device.uuid);
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
                                odyssey_hub_common::config::device_offsets_save_async(&guard).await
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
                                let _ = tx.send(u32::from(ms));
                            } else {
                                let (tx, _rx) = watch::channel(u32::from(ms));
                                let _ = tx.send(u32::from(ms));
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
                        let port = if port == 0 { odyssey_protocol::Port::Nf } else { odyssey_protocol::Port::Wf };
                        if !device.events_connected {
                            let _ = reply.send(Err("events transport not connected".into()));
                            continue;
                        }
                        let Some(dev) = connections.events_device.as_ref().cloned() else {
                            let _ = reply.send(Err("events device not available".into()));
                            continue;
                        };
                        // Pause sensor streams to avoid PAG mutex contention on the firmware.
                        // obj_loop holds the PAG mutex during get_objects(); if a ReadRegister
                        // arrives while that lock is held, recv_loop blocks for the full
                        // get_objects() duration. We send DisableAll first (an acknowledged
                        // round-trip) so obj_loop stops before ReadRegister reaches the firmware.
                        //
                        // We pause once for the whole batch (first read in the sequence) and
                        // restart via a 100 ms debounce after the last read, rather than
                        // cycling streams around every individual read.
                        if has_sensor_streams {
                            streams_paused = true;
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
                        // Arm/reset the debounce timer. Streams will restart once no register
                        // ops arrive for 100 ms (i.e., after the last read in the batch).
                        if streams_paused {
                            streams_resume_sleep.as_mut().reset(
                                tokio::time::Instant::now() + Duration::from_millis(100),
                            );
                            streams_resume_armed = true;
                        }
                    }
                    Some(DeviceTaskMessage::WriteRegister { port, bank, address, data, reply }) => {
                        let port = if port == 0 { odyssey_protocol::Port::Nf } else { odyssey_protocol::Port::Wf };
                        if !device.events_connected {
                            let _ = reply.send(Err("events transport not connected".into()));
                            continue;
                        }
                        let Some(dev) = connections.events_device.as_ref().cloned() else {
                            let _ = reply.send(Err("events device not available".into()));
                            continue;
                        };
                        // Pause streams while writing registers for the same reason as ReadRegister.
                        // Uses the same once-per-batch pause + debounce-restart pattern.
                        if has_sensor_streams {
                            streams_paused = true;
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
                        if streams_paused {
                            streams_resume_sleep.as_mut().reset(
                                tokio::time::Instant::now() + Duration::from_millis(100),
                            );
                            streams_resume_armed = true;
                        }
                    }
                    Some(DeviceTaskMessage::ReadConfig { kind, reply }) => {
                        let ck = match kind {
                            0 => ConfigKind::ImpactThreshold,
                            1 => ConfigKind::SuppressMs,
                            _ => {
                                let _ = reply.send(Err(format!("unknown config kind: {kind}")));
                                continue;
                            }
                        };
                        // read_config uses packet protocol — works over BLE and USB alike.
                        // Only read_all_config (bulk camera data) is gated on USB transport mode.
                        match connections.events_device.as_ref().cloned() { Some(dev) => {
                            if has_sensor_streams {
                                streams_paused = true;
                                sensor_streams = SelectAll::new();
                                has_sensor_streams = false;
                                if let Err(e) = dev.clear_all_streams().await {
                                    warn!("clear_all_streams failed before ReadConfig for {:02x?}: {}", device.uuid, e);
                                }
                            }
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
                            if streams_paused {
                                streams_resume_sleep.as_mut().reset(
                                    tokio::time::Instant::now() + Duration::from_millis(100),
                                );
                                streams_resume_armed = true;
                            }
                        } _ => if let Some(dev) = connections.control_device.as_ref() {
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
                        }}
                    }
                    Some(DeviceTaskMessage::WriteConfig { kind, value, reply }) => {
                        let gc = match kind {
                            0 => GeneralConfig::ImpactThreshold(value),
                            1 => GeneralConfig::SuppressMs(value),
                            _ => {
                                let _ = reply.send(Err(format!("unknown config kind: {kind}")));
                                continue;
                            }
                        };
                        // write_config uses packet protocol — works over BLE and USB alike.
                        match connections.events_device.as_ref().cloned() { Some(dev) => {
                            if has_sensor_streams {
                                streams_paused = true;
                                sensor_streams = SelectAll::new();
                                has_sensor_streams = false;
                                if let Err(e) = dev.clear_all_streams().await {
                                    warn!("clear_all_streams failed before WriteConfig for {:02x?}: {}", device.uuid, e);
                                }
                            }
                            let result = match tokio::time::timeout(Duration::from_secs(3), dev.write_config(gc)).await {
                                Ok(r) => r.map_err(|e| e.to_string()),
                                Err(_) => Err("write_config timed out".into()),
                            };
                            let _ = reply.send(result);
                            if streams_paused {
                                streams_resume_sleep.as_mut().reset(
                                    tokio::time::Instant::now() + Duration::from_millis(100),
                                );
                                streams_resume_armed = true;
                            }
                        } _ => if let Some(dev) = connections.control_device.as_ref() {
                            let result = match tokio::time::timeout(Duration::from_secs(3), dev.write_config_ctrl(gc)).await {
                                Ok(r) => r.map_err(|e| e.to_string()),
                                Err(_) => Err("write_config_ctrl timed out".into()),
                            };
                            let _ = reply.send(result);
                        } else {
                            let _ = reply.send(Err("no device available for write_config".into()));
                        }}
                    }
                    Some(DeviceTaskMessage::FlashSettings { reply }) => {
                        // flash_settings uses packet protocol — works over BLE and USB alike.
                        if let Some(dev) = connections.events_device.as_ref() {
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
                                    let (success, paired_address, paired_name, error) = match dev_clone.wait_pairing_event().await {
                                        Ok(device) => (true, device.uuid, device.name.to_string(), String::new()),
                                        Err(e) => (false, [0; 6], String::new(), e.to_string()),
                                    };
                                    pairing_flag.store(false, std::sync::atomic::Ordering::Relaxed);
                                    let event = Event::DeviceEvent(
                                        DeviceEvent(
                                            dev_meta,
                                            DeviceEventKind::PairingResult {
                                                success,
                                                paired_address,
                                                paired_name,
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
                        match connections.control_device.clone() { Some(dev) => {
                            info!("SetTransportMode: usb_mode={} for device {:02x?}", usb_mode, device.uuid);
                            let r = match tokio::time::timeout(Duration::from_secs(3), dev.set_transport_mode(usb_mode)).await {
                                Ok(Ok(mode)) => Ok(matches!(mode, odyssey_protocol::control::device::TransportMode::Usb)),
                                Ok(Err(e)) => Err(e.to_string()),
                                Err(_) => Err("set_transport_mode timed out".into()),
                            };
                            info!("SetTransportMode result for {:02x?}: {:?}", device.uuid, r);
                            // The firmware's SetTransportMode handler persists the mode itself
                            // via persist_transport_mode, so no separate FlashSettings is needed.
                            // Sending FlashSettings would corrupt legacy board settings because
                            // atslite-common's GeneralSettings uses a different CBOR field layout
                            // than common's GeneralSettings (transport_mode at field #0 vs #7).
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
                        } _ => {
                            warn!("SetTransportMode: no control device for {:02x?}", device.uuid);
                            let _ = reply.send(Err("no control device available".into()));
                        }}
                    }
                    Some(DeviceTaskMessage::SetDeviceName { name, reply }) => {
                        info!("SetDeviceName: name={:?} for device {:02x?}", name, device.uuid);
                        let r = if let Some(dev) = connections.control_device.as_ref() {
                            // USB EP0 path
                            match tokio::time::timeout(Duration::from_secs(5), dev.set_name(&name)).await {
                                Ok(Ok(())) => Ok(()),
                                Ok(Err(e)) => Err(e.to_string()),
                                Err(_) => Err("set_device_name timed out".into()),
                            }
                        } else if let Some(dev) = connections.events_device.as_ref() {
                            // BLE mux path
                            let hs_name = odyssey_protocol::DeviceName::from_str_lossy(&name);
                            match tokio::time::timeout(Duration::from_secs(5), dev.request(PacketData::SetDeviceName(hs_name))).await {
                                Ok(Ok(PacketData::SetDeviceNameResponse(odyssey_protocol::SetDeviceNameResult::Ok()))) => Ok(()),
                                Ok(Ok(_)) => Err("unexpected response to SetDeviceName".into()),
                                Ok(Err(e)) => Err(e.to_string()),
                                Err(_) => Err("set_device_name timed out".into()),
                            }
                        } else {
                            warn!("SetDeviceName: no device for {:02x?}", device.uuid);
                            Err("no device available".into())
                        };
                        info!("SetDeviceName result for {:02x?}: {:?}", device.uuid, r);
                        if r.is_ok() {
                            device.name = Device::name_bytes(&name);
                            bond_names.lock().await.insert(device.uuid, name.clone());
                            if let Some(ref hub) = hub_device {
                                let hs = odyssey_protocol::DeviceName::from_str_lossy(&name);
                                if let Err(e) = hub.update_bond_name(device.uuid, hs.into()).await {
                                    warn!("update_bond_name failed for {:02x?}: {}", device.uuid, e);
                                }
                            }
                            let _ = message_channel.send(Message::UpdateName(device.uuid, name)).await;
                        }
                        let _ = reply.send(r);
                    }
                    Some(DeviceTaskMessage::AddEventsDevice(events_vm_device, ble_proto_version)) => {
                        // Update protocol_version with the BLE device's version. This is necessary
                        // when the handler was created for a USB-in-BLE-mode device, which starts
                        // with protocol_version = [0,0,0] because events weren't readable over USB.
                        if ble_proto_version > protocol_version {
                            info!(
                                "AddEventsDevice: updating protocol_version for {:02x?}: {}.{}.{} -> {}.{}.{}",
                                device.uuid,
                                protocol_version[0], protocol_version[1], protocol_version[2],
                                ble_proto_version[0], ble_proto_version[1], ble_proto_version[2],
                            );
                            protocol_version = ble_proto_version;
                        }

                        // The BLE hub sends a new snapshot (and thus AddEventsDevice) every few
                        // seconds even while the device is already streaming.  Re-running
                        // start_sensor_streams + read_all_config blocks the select loop for
                        // 5-10 s on every snapshot, causing periodic BLE lag.  Skip if we are
                        // already streaming — the first invocation already set everything up.
                        if has_sensor_streams {
                            info!(
                                "AddEventsDevice for {:02x?}: already streaming, ignoring redundant snapshot",
                                device.uuid
                            );
                            continue;
                        }

                        info!(
                            "AddEventsDevice - adding BLE mux events device for {:02x?}",
                            device.uuid
                        );

                        // Store the events device
                        connections.events_device = Some(events_vm_device.clone());

                        // Start all sensor streams with the new events device
                        sensor_streams = start_sensor_streams(&events_vm_device, device.uuid, protocol_version).await;

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
                                stereo_iso = cfg.stereo_iso;
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

                        // A unit with no cable has no USB control to read the net core
                        // through; ask over the packet stream instead.
                        if connections.control_device.is_none() && device.netcore_release == 0 {
                            spawn_packet_netcore_probe(&self_tx, &events_vm_device, protocol_version);
                        }

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
                                // Note: protocol_version gates events-protocol features (like battery
                                // stream) and was set from the events protocol's ReadVersionResponse at
                                // spawn time. The control channel's protocol_semver is unrelated and
                                // must not override it here.
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
                            let is_usb = mode == odyssey_protocol::control::device::TransportMode::Usb;
                            device.events_transport = if is_usb {
                                odyssey_hub_common::device::EventsTransport::Wired
                            } else {
                                odyssey_hub_common::device::EventsTransport::Bluetooth
                            };
                            if is_usb {
                                // USB transport mode: events come from USB, BLE not needed.
                                device.events_connected = true;
                                device.capabilities.insert(DeviceCapabilities::EVENTS);
                            } else if connections.events_device.is_some() {
                                // BLE transport mode but BLE events are already streaming —
                                // preserve EVENTS capability and events_connected as-is.
                            } else {
                                // BLE transport mode, no BLE events yet.
                                device.events_connected = false;
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
                    Some(DeviceTaskMessage::SetNetCoreRelease(release)) => {
                        let was_known = device.capabilities.contains(DeviceCapabilities::NETCORE_KNOWN);
                        if device.netcore_release != release || !was_known {
                            info!(
                                "Net core of {:02x?} reports release {}",
                                device.uuid, release
                            );
                            device.netcore_release = release;
                            device.capabilities.insert(DeviceCapabilities::NETCORE_KNOWN);
                            let _ = message_channel.send(Message::UpdateDevice(device.clone())).await;
                        }
                    }
                    Some(DeviceTaskMessage::RemoveControlDevice) => {
                        info!(
                            "RemoveControlDevice - removing USB control for {:02x?}",
                            device.uuid
                        );

                        connections.control_device = None;
                        device.capabilities.remove(DeviceCapabilities::CONTROL);
                        // DFU interface is only accessible via USB; clear it on USB disconnect.
                        device.capabilities.remove(DeviceCapabilities::DFU);

                        // A device with an active BLE events connection has genuinely independent
                        // events that survive USB disconnect. A purely wired device's events are
                        // co-located with the control connection that just disappeared, so exit.
                        let has_independent_ble_events = connections.events_device.is_some()
                            && device.events_transport
                                == odyssey_hub_common::device::EventsTransport::Bluetooth
                            && device.events_connected;

                        if has_independent_ble_events {
                            device.transport = odyssey_hub_common::device::Transport::UsbMux;
                            // The net core can only be read over USB control, and it may be
                            // replaced while the cable is out, so the old reading is stale.
                            device.netcore_release = 0;
                            device.capabilities.remove(DeviceCapabilities::NETCORE_KNOWN);
                            if let Some(events) = &connections.events_device {
                                spawn_packet_netcore_probe(&self_tx, events, protocol_version);
                            }
                            // No longer a direct-USB handler — USB control is gone. Remove from
                            // the set so the next USB re-plug sends AddControlDevice correctly.
                            direct_usb_handler_uuids.lock().await.remove(&device.uuid);
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
            () = &mut streams_resume_sleep, if streams_resume_armed => {
                // Debounce expired: no register ops arrived for 100 ms.
                // Restart sensor streams now that the firmware's PAG mutex is free.
                streams_resume_armed = false;
                streams_paused = false;
                if !has_sensor_streams
                    && let Some(dev) = connections.events_device.as_ref().cloned() {
                        info!(
                            "Register access batch done, restarting sensor streams for {:02x?}",
                            device.uuid
                        );
                        sensor_streams = start_sensor_streams(&dev, device.uuid, protocol_version).await;
                        has_sensor_streams = !sensor_streams.is_empty();
                    }
            }
            () = &mut streams_retry_sleep, if !has_sensor_streams && !streams_paused && connections.events_device.is_some() => {
                info!("Retrying sensor streams for {:02x?}", device.uuid);
                sensor_streams = start_sensor_streams(
                    connections.events_device.as_ref().unwrap(),
                    device.uuid,
                    protocol_version,
                ).await;
                has_sensor_streams = !sensor_streams.is_empty();
                streams_retry_sleep.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(5));
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
                        info!("Sensor streams ended for device {:02x?}, will retry in 5s", device.uuid);
                        // Don't break — keep the handler alive so the device stays
                        // in the list and the user can still trigger a DFU update.
                        // The USB scan loop will send RemoveControlDevice when the
                        // device is physically unplugged.
                        has_sensor_streams = false;
                        sensor_streams = SelectAll::new();
                        streams_retry_sleep.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(5));
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
    let disconnect_device = device.clone();
    let _ = message_channel
        .send(Message::Disconnect(disconnect_device))
        .await;
}
