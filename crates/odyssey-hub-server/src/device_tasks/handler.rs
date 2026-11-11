mod commands;
mod connection_changes;
mod images;
mod sensor_control;
#[cfg(test)]
mod tests;
mod zero_offset;

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{Fuse, StreamExt as _};
use odyssey_hub_common::device::{Device, DeviceCapabilities};
use odyssey_usb::device::{GeneralSettings, MuxDevice, VmDevice};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::{ReceiverStream, WatchStream};
use tracing::{debug, info, warn};

use super::context::DeviceTaskContext;
use super::messages::{DeviceTaskMessage, Message};
use super::sensor_streams::{DeviceStreamEvent, SensorStreams, StreamRestartTimers};
use super::tracking::DeviceTracker;
use super::transport_selection::TransportSelection;

/// Default budget for one request/response round trip to a unit.
const CALL_TIMEOUT: Duration = Duration::from_secs(3);

/// Budget for calls that make the unit touch flash or radio, which answer slowly.
const SLOW_CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Budget for reading the whole config over the control endpoint.
const CONTROL_CONFIG_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// A unit's links. Control and events can be separate `VmDevice`s.
pub(super) struct DeviceConnections {
    /// Direct USB control connection.
    pub(super) control_device: Option<VmDevice>,
    /// Event streams, over direct USB or the BLE mux.
    pub(super) events_device: Option<VmDevice>,
}

/// The BLE link of a unit whose firmware picks its own transport, held while USB carries
/// the reports. Its streams stay subscribed, so when the cable is pulled the reports the
/// firmware sends over BLE straight away are forwarded before the USB scan notices.
struct StandbyLink {
    device: VmDevice,
    sensors: SensorStreams,
}

/// Everything a handler needs to start.
pub(super) struct DeviceHandlerInit {
    pub(super) connections: DeviceConnections,
    pub(super) device: Device,
    pub(super) control_rx: mpsc::Receiver<DeviceTaskMessage>,
    pub(super) context: DeviceTaskContext,
    /// Events protocol version, which gates features such as the battery stream.
    pub(super) protocol_version: [u16; 3],
    /// The dongle the unit is reached through, if any, so renames can reach its bond cache.
    pub(super) hub_device: Option<(MuxDevice, [u8; 6])>,
    /// Lets the handler start work that reports back to itself without keeping its own
    /// channel open after every other owner has dropped theirs.
    pub(super) self_tx: mpsc::WeakSender<DeviceTaskMessage>,
}

/// Per-device handler: receives packets from the link, processes control messages,
/// watches for shot-delay changes, and emits events.
///
/// The task ends when the unit has no connection left that it can serve, and reports
/// that to the server with a final [`Message::Disconnect`].
pub(super) struct DeviceHandler {
    context: DeviceTaskContext,
    device: Device,
    connections: DeviceConnections,
    /// Only for automatic transport selection, and only while USB is the events device.
    standby: Option<StandbyLink>,
    protocol_version: [u16; 3],
    hub_device: Option<(MuxDevice, [u8; 6])>,
    events_owner: Option<Arc<()>>,
    self_tx: mpsc::WeakSender<DeviceTaskMessage>,
    tracker: DeviceTracker,
    sensors: SensorStreams,
    stream_timers: StreamRestartTimers,
    image_session: Option<images::ImageSession>,
    image_reboot_required: bool,
    shot_delay_stream: Fuse<WatchStream<u32>>,
    control_stream: Fuse<ReceiverStream<DeviceTaskMessage>>,
}

impl DeviceHandler {
    /// Run a handler for `init` on its own task. The task is owned by the caller, which
    /// watches for it finishing (see `UsbDeviceManager::forget_finished_handlers`).
    pub(super) fn spawn(init: DeviceHandlerInit) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move { Self::start(init).await.run().await })
    }

    /// Reads the unit's config and starts its sensor streams. That can take seconds, so it
    /// runs inside the spawned task instead of in the caller.
    async fn start(init: DeviceHandlerInit) -> Self {
        let DeviceHandlerInit {
            connections,
            device,
            control_rx,
            context,
            protocol_version,
            hub_device,
            self_tx,
        } = init;
        let uuid = device.uuid;
        info!("Starting device handler for device UUID {uuid:02x?}");

        let shot_delay_rx = subscribe_to_shot_delay(&context, uuid).await;
        info!(
            "Initial shot delay for {uuid:02x?} = {} ms",
            *shot_delay_rx.borrow()
        );

        // All config (camera models, impact threshold, etc.) is readable over both
        // USB and BLE via packet protocol. Prefer the events device and fall back to
        // the control device only when no events device is present.
        let general_settings = read_initial_settings(&connections, uuid).await;

        info!("Setting up camera models for device {uuid:02x?}");
        if general_settings.is_none() {
            // Replaced by the real models when AddEventsDevice delivers a config.
            info!("Device {uuid:02x?} starting in BLE mode with dummy camera models");
        }
        let tracker = DeviceTracker::new(general_settings.as_ref());

        info!("Starting sensor streams for device {uuid:02x?}");
        let sensors = if let Some(events_device) = connections.events_device.as_ref() {
            SensorStreams::start(events_device, uuid, protocol_version).await
        } else {
            info!("Skipping sensor streams for device {uuid:02x?} (no events device)");
            SensorStreams::stopped()
        };
        // Without sensor streams (e.g. a unit in BLE mode) the device stays connected for
        // control (DFU, etc.), and the retry timer keeps looking for events.
        info!(
            "Device {uuid:02x?} has_sensor_streams: {}",
            sensors.is_running()
        );

        Self {
            context,
            device,
            connections,
            standby: None,
            protocol_version,
            hub_device,
            events_owner: None,
            self_tx,
            tracker,
            sensors,
            stream_timers: StreamRestartTimers::new(),
            image_session: None,
            image_reboot_required: false,
            shot_delay_stream: WatchStream::new(shot_delay_rx).fuse(),
            control_stream: ReceiverStream::new(control_rx).fuse(),
        }
    }

    async fn run(mut self) {
        info!(
            "Device handler task entering main loop for {:02x?}, has_sensor_streams={}",
            self.device.uuid,
            self.sensors.is_running()
        );

        loop {
            tokio::select! {
                biased;
                maybe_message = self.control_stream.next() => {
                    info!(
                        "control_stream received '{:?}' for {:02x?}",
                        maybe_message, self.device.uuid
                    );
                    let Some(message) = maybe_message else {
                        info!("Control channel closed for {:02x?}", self.device.uuid);
                        break;
                    };
                    if self.on_control_message(message).await.is_break() {
                        break;
                    }
                }
                image_event = images::next_image_event(&mut self.image_session),
                    if self.image_session.is_some() =>
                {
                    self.on_image_event(image_event).await;
                }
                Some(new_delay) = self.shot_delay_stream.next() => {
                    self.on_shot_delay_changed(new_delay).await;
                }
                () = &mut self.stream_timers.resume_sleep, if self.stream_timers.resume_armed && !self.image_reboot_required => {
                    self.on_streams_resume_due().await;
                }
                () = &mut self.stream_timers.retry_sleep,
                    if !self.stream_timers.paused && self.has_stopped_streams() =>
                {
                    self.on_streams_retry_due().await;
                }
                maybe_event = self.sensors.next_event(), if self.sensors.is_running() => {
                    self.on_sensor_event(maybe_event).await;
                }
                maybe_event = next_standby_event(&mut self.standby),
                    if self.standby.as_ref().is_some_and(|link| link.sensors.is_running()) =>
                {
                    self.on_standby_event(maybe_event).await;
                }
                else => {
                    warn!(
                        "select! else branch fired for {:02x?} — all branches disabled, breaking loop",
                        self.device.uuid
                    );
                    break;
                }
            }
        }

        self.stop_image_session(Some(tonic::Status::unavailable("device disconnected")))
            .await;

        info!(
            "Device handler task ending for {:02x?}, sending disconnect",
            self.device.uuid
        );
        let disconnect_device = self.device.clone();
        self.context
            .send_message(Message::Disconnect(disconnect_device))
            .await;
    }

    /// Dispatch one control message. `Break` ends the handler task.
    async fn on_control_message(&mut self, message: DeviceTaskMessage) -> ControlFlow<()> {
        // Sensor operations and link replacement must never race camera mode ownership.
        // End capture explicitly before allowing the existing supervised control path
        // to pause streams, change transport, or replace an endpoint.
        if self.image_session.is_some()
            && matches!(
                &message,
                DeviceTaskMessage::ReadRegister { .. }
                    | DeviceTaskMessage::WriteRegister { .. }
                    | DeviceTaskMessage::ReadConfig { .. }
                    | DeviceTaskMessage::WriteConfig { .. }
                    | DeviceTaskMessage::FlashSettings { .. }
                    | DeviceTaskMessage::OtaExchange { .. }
                    | DeviceTaskMessage::SetTransportMode { .. }
                    | DeviceTaskMessage::AddControlDevice { .. }
                    | DeviceTaskMessage::RemoveControlDevice
            )
        {
            self.stop_image_session(Some(tonic::Status::aborted(
                "image session ended for device control or transport change",
            )))
            .await;
        }
        if self.image_reboot_required {
            let refused = "Legacy image mode requires reboot/power-cycle and reconnect before sensor settings can be accessed.";
            match message {
                DeviceTaskMessage::ReadRegister { reply, .. }
                | DeviceTaskMessage::ReadConfig { reply, .. } => {
                    let _ = reply.send(Err(refused.to_owned()));
                    return ControlFlow::Continue(());
                }
                DeviceTaskMessage::WriteRegister { reply, .. }
                | DeviceTaskMessage::WriteConfig { reply, .. }
                | DeviceTaskMessage::FlashSettings { reply } => {
                    let _ = reply.send(Err(refused.to_owned()));
                    return ControlFlow::Continue(());
                }
                _ => {}
            }
        }
        match message {
            DeviceTaskMessage::Reboot { reply } => {
                self.stop_image_session(None).await;
                let result = match self.connections.control_device.clone() {
                    Some(device) => tokio::time::timeout(CALL_TIMEOUT, device.reboot()).await
                        .map_err(|error| format!("Device reboot acknowledgement timed out: {error}; power-cycle and reconnect if it does not return."))
                        .and_then(|result| result.map_err(|error| error.to_string())),
                    None => Err("No USB control endpoint; power-cycle the device and reconnect.".to_owned()),
                };
                let accepted = result.is_ok();
                let _ = reply.send(result);
                if accepted {
                    return ControlFlow::Break(());
                }
            }
            DeviceTaskMessage::StartImages {
                config,
                renewals: super::ImageRenewals(renewals),
                reply,
            } => {
                let result = self.start_image_session(config, renewals).await;
                let _ = reply.send(Ok(result));
            }
            DeviceTaskMessage::SetTransportMode { usb_mode, reply } => {
                return self.on_set_transport_mode(usb_mode, reply).await;
            }
            DeviceTaskMessage::RemoveControlDevice => {
                return self.on_remove_control_device().await;
            }
            DeviceTaskMessage::RemoveEventsDevice(owner) => {
                return self.on_remove_events_device(owner).await;
            }
            DeviceTaskMessage::ResetZero => self.on_reset_zero().await,
            DeviceTaskMessage::SaveZero => self.on_save_zero().await,
            DeviceTaskMessage::Zero(translation, target, tracking_event) => {
                self.on_zero(translation, target, tracking_event).await;
            }
            DeviceTaskMessage::ClearZero => self.on_clear_zero().await,
            DeviceTaskMessage::SetShotDelay(ms) => self.on_set_shot_delay(ms).await,
            DeviceTaskMessage::WriteVendor(tag, data) => self.on_write_vendor(tag, &data).await,
            DeviceTaskMessage::OtaExchange { frame, reply } => {
                let result = self.on_ota_exchange(&frame).await;
                let _ = reply.send(result);
            }
            DeviceTaskMessage::ReadRegister {
                port,
                bank,
                address,
                reply,
            } => self.on_read_register(port, bank, address, reply).await,
            DeviceTaskMessage::WriteRegister {
                port,
                bank,
                address,
                data,
                reply,
            } => {
                self.on_write_register(port, bank, address, data, reply)
                    .await;
            }
            DeviceTaskMessage::ReadConfig { kind, reply } => {
                self.on_read_config(kind, reply).await;
            }
            DeviceTaskMessage::WriteConfig { kind, value, reply } => {
                self.on_write_config(kind, value, reply).await;
            }
            DeviceTaskMessage::FlashSettings { reply } => self.on_flash_settings(reply).await,
            DeviceTaskMessage::StartPairing { timeout_ms, reply } => {
                self.on_start_pairing(timeout_ms, reply).await;
            }
            DeviceTaskMessage::CancelPairing { reply } => self.on_cancel_pairing(reply).await,
            DeviceTaskMessage::ClearBond { reply } => self.on_clear_bond(reply).await,
            DeviceTaskMessage::SetDeviceName { name, reply } => {
                self.on_set_device_name(name, reply).await;
            }
            DeviceTaskMessage::AddEventsDevice {
                device,
                protocol_version,
                hub,
                address,
                owner,
            } => {
                self.on_add_events_device(device, protocol_version, hub, address, owner)
                    .await;
            }
            DeviceTaskMessage::AddControlDevice {
                device,
                dfu,
                events_protocol_version,
                transport_selection,
            } => {
                self.on_add_control_device(
                    device,
                    dfu,
                    events_protocol_version,
                    transport_selection,
                )
                .await;
            }
            DeviceTaskMessage::SetNetCoreRelease(release) => {
                self.on_net_core_release(release).await;
            }
        }
        ControlFlow::Continue(())
    }

    async fn on_shot_delay_changed(&mut self, new_delay: u32) {
        info!(
            "Shot delay updated for {:02x?} = {} ms",
            self.device.uuid, new_delay
        );
        self.context
            .device_shot_delays
            .write()
            .await
            .insert(self.device.uuid, new_delay as u16);
    }

    async fn on_set_shot_delay(&mut self, ms: u16) {
        let uuid = self.device.uuid;
        debug!("SetShotDelay requested for {uuid:02x?} = {ms} ms");
        let mut watchers = self.context.shot_delay_watch.write().await;
        self.context
            .device_shot_delays
            .write()
            .await
            .insert(uuid, ms);
        if let Some(sender) = watchers.get(&uuid) {
            sender.send_replace(u32::from(ms));
        } else {
            let (sender, _receiver) = watch::channel(u32::from(ms));
            sender.send_replace(u32::from(ms));
            watchers.insert(uuid, sender);
        }
    }

    /// The device entry's `AUTO_TRANSPORT` capability is the handler's record of it, so
    /// clients and the handler cannot disagree.
    fn transport_selection(&self) -> TransportSelection {
        if self
            .device
            .capabilities
            .contains(DeviceCapabilities::AUTO_TRANSPORT)
        {
            TransportSelection::Automatic
        } else {
            TransportSelection::Manual
        }
    }

    fn set_transport_selection(&mut self, selection: TransportSelection) {
        match selection {
            TransportSelection::Automatic => self
                .device
                .capabilities
                .insert(DeviceCapabilities::AUTO_TRANSPORT),
            TransportSelection::Manual => self
                .device
                .capabilities
                .remove(DeviceCapabilities::AUTO_TRANSPORT),
        }
    }

    /// A link is attached whose sensor streams are not running.
    fn has_stopped_streams(&self) -> bool {
        if self.image_reboot_required {
            return false;
        }
        let events_stopped = self.connections.events_device.is_some() && !self.sensors.is_running();
        let standby_stopped = self
            .standby
            .as_ref()
            .is_some_and(|link| !link.sensors.is_running());
        events_stopped || standby_stopped
    }

    /// The links a packet-protocol request can take, in the order to try them: the events
    /// device, then the standby link.
    fn packet_links(&self) -> Vec<VmDevice> {
        self.connections
            .events_device
            .iter()
            .chain(self.standby.as_ref().map(|link| &link.device))
            .cloned()
            .collect()
    }
}

/// Subscribe to `uuid`'s shot-delay watch, creating the watch if there is none.
async fn subscribe_to_shot_delay(
    context: &DeviceTaskContext,
    uuid: [u8; 6],
) -> watch::Receiver<u32> {
    let mut watchers = context.shot_delay_watch.write().await;
    if let Some(sender) = watchers.get(&uuid) {
        return sender.subscribe();
    }
    let current = u32::from(
        context
            .device_shot_delays
            .read()
            .await
            .get(&uuid)
            .copied()
            .unwrap_or(0),
    );
    let (sender, receiver) = watch::channel(current);
    watchers.insert(uuid, sender);
    receiver
}

async fn read_initial_settings(
    connections: &DeviceConnections,
    uuid: [u8; 6],
) -> Option<GeneralSettings> {
    if let Some(events_device) = connections.events_device.as_ref() {
        info!("Reading config for device {uuid:02x?} from events device");
        let read = tokio::time::timeout(SLOW_CALL_TIMEOUT, events_device.read_all_config()).await;
        return log_config_read(read, uuid, "", "read_all_config");
    }
    if let Some(control_device) = connections.control_device.as_ref() {
        info!("Reading config for device {uuid:02x?} via control plane (no events device)");
        let read = tokio::time::timeout(
            CONTROL_CONFIG_READ_TIMEOUT,
            control_device.read_all_config_ctrl(),
        )
        .await;
        return log_config_read(read, uuid, " via control plane", "read_all_config_ctrl");
    }
    info!("Skipping config for device {uuid:02x?} (no events or control device)");
    None
}

fn log_config_read(
    read: Result<odyssey_usb::Result<GeneralSettings>, tokio::time::error::Elapsed>,
    uuid: [u8; 6],
    via: &str,
    call_name: &str,
) -> Option<GeneralSettings> {
    match read {
        Ok(Ok(settings)) => {
            info!("Successfully read config for device {uuid:02x?}{via}");
            Some(settings)
        }
        Ok(Err(e)) => {
            warn!("Failed to read config for device {uuid:02x?}{via}: {e}");
            None
        }
        Err(_) => {
            warn!("{call_name} timed out for device {uuid:02x?}, proceeding with defaults");
            None
        }
    }
}

/// Run `call` for at most `limit`, flattening a failure or a timeout into the error string
/// that RPC replies carry.
async fn call_with_timeout<T, E: ToString>(
    limit: Duration,
    call_name: &str,
    call: impl Future<Output = Result<T, E>>,
) -> Result<T, String> {
    match tokio::time::timeout(limit, call).await {
        Ok(result) => result.map_err(|e| e.to_string()),
        Err(_) => Err(format!("{call_name} timed out")),
    }
}

/// [`call_with_timeout`] over each of `links` in turn until one succeeds. The error is the
/// last link's. While the cable is in, the first link is USB and a request caught by the
/// unplug fails there before the scan notices, while the unit already listens on BLE.
async fn call_over_links<T, Call, Fut>(
    links: &[VmDevice],
    limit: Duration,
    call_name: &str,
    call: Call,
) -> Result<T, String>
where
    Call: Fn(VmDevice) -> Fut,
    Fut: Future<Output = odyssey_usb::Result<T>>,
{
    let mut result = Err(format!("{call_name}: no device available"));
    for (attempt, link) in links.iter().enumerate() {
        if let Err(e) = &result
            && attempt > 0
        {
            warn!("{call_name} failed ({e}), trying the next link");
        }
        result = call_with_timeout(limit, call_name, call(link.clone())).await;
        if result.is_ok() {
            break;
        }
    }
    result
}

async fn next_standby_event(standby: &mut Option<StandbyLink>) -> Option<DeviceStreamEvent> {
    match standby {
        Some(link) => link.sensors.next_event().await,
        None => std::future::pending().await,
    }
}
