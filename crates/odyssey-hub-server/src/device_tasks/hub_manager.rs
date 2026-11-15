mod commands;
mod dongle;
mod snapshot;

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use backon::{ConstantBuilder, Retryable as _};
use futures::stream::{self, BoxStream, StreamExt as _};
use futures_concurrency::stream::Merge as _;
use odyssey_hub_common::events::Event;
use odyssey_usb::Result;
use odyssey_usb::device::{MuxDevice, PairingEvent};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{MissedTickBehavior, interval};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info, warn};

use self::commands::ControlCommand;
use self::dongle::DongleIdentity;
use super::context::{DeviceAddress, DeviceHandles, DeviceTaskContext};
use super::messages::{DeviceTaskMessage, DongleTaskMessage, Message};

/// Consecutive failed connect attempts to a dongle-reported device before the hub
/// manager waits for a fresh snapshot instead of retrying on its own.
const MAX_HUB_CONNECT_ATTEMPTS: u8 = 5;

/// How often devices whose connect failed are retried.
const RETRY_TICK_INTERVAL: Duration = Duration::from_secs(2);

const INITIAL_DEVICE_REQUEST_ATTEMPTS: u8 = 3;
const INITIAL_DEVICE_REQUEST_RETRY_DELAY: Duration = Duration::from_millis(200);

/// Capacity of the dongle's command channel.
const DONGLE_COMMAND_CHANNEL_CAPACITY: usize = 32;

/// The BLE addresses a dongle currently reports as connected.
type DeviceSnapshot = heapless::Vec<DeviceAddress, { odyssey_protocol::mux::MAX_DEVICES }>;

/// Everything a hub manager needs to start.
pub(super) struct HubManagerInit {
    pub(super) hub: MuxDevice,
    pub(super) serial: String,
    pub(super) context: DeviceTaskContext,
    pub(super) device_handles: DeviceHandles,
    pub(super) shutdown: oneshot::Receiver<()>,
}

/// Events from the hub manager's merged stream.
enum HubEvent {
    Snapshot(Result<DeviceSnapshot>),
    /// Timer tick. Re-drives snapshot processing for devices that failed to connect.
    Retry,
    Command(ControlCommand),
}

struct ManagedDevice {
    uuid: DeviceAddress,
    commands: mpsc::Sender<DeviceTaskMessage>,
}

/// Manages one dongle: tracks which units it reports over Bluetooth, gives each a
/// handler (or attaches the dongle's events to the handler a cable already created), and
/// serves the dongle's own commands (pairing, bonds).
pub(super) struct HubManager {
    context: DeviceTaskContext,
    device_handles: DeviceHandles,
    hub: MuxDevice,
    serial: String,
    dongle: DongleIdentity,
    /// Unique to this manager incarnation, even when a dongle reconnects at the same address.
    events_owner: Arc<()>,
    /// Devices managed by this hub, for cleanup on exit. Keyed by BLE address (not UUID) so
    /// snapshot diffs work even when a device's UUID differs from its BLE address. The
    /// value retains the original handler so cleanup cannot detach a newer connection.
    managed_devices: HashMap<DeviceAddress, ManagedDevice>,
    last_snapshot: DeviceSnapshot,
    /// BLE address -> consecutive failed connect attempts.
    failed_adds: HashMap<DeviceAddress, u8>,
}

impl HubManager {
    /// Run the manager on its own task. The task ends when the hub disconnects. The caller
    /// watches for that (see `UsbDeviceManager::on_hub_device`).
    pub(super) fn spawn(init: HubManagerInit) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            tracing::info!("Hub manager task started for hub {}", init.serial);
            let (manager, commands, shutdown) = Self::register(init).await;
            manager.run(commands, shutdown).await;
        })
    }

    /// Register the dongle in the dongle list and open its command channel.
    async fn register(
        init: HubManagerInit,
    ) -> (
        Self,
        mpsc::Receiver<DongleTaskMessage>,
        oneshot::Receiver<()>,
    ) {
        let HubManagerInit {
            hub,
            serial,
            context,
            device_handles,
            shutdown,
        } = init;
        let dongle = DongleIdentity::read(&hub, &serial).await;
        let manager = Self {
            context,
            device_handles,
            hub,
            serial,
            dongle,
            events_owner: Arc::new(()),
            managed_devices: HashMap::new(),
            last_snapshot: DeviceSnapshot::new(),
            failed_adds: HashMap::new(),
        };

        let bonded_devices = manager.list_bonds_at_startup().await;
        let (commands_tx, commands_rx) = mpsc::channel(DONGLE_COMMAND_CHANNEL_CAPACITY);
        manager
            .context
            .send_message(Message::DongleConnect(
                manager.dongle.info(Vec::new(), bonded_devices),
                commands_tx,
            ))
            .await;
        tracing::info!("Registered dongle '{}' in dongle list", manager.dongle.id);
        (manager, commands_rx, shutdown)
    }

    async fn run(
        mut self,
        commands: mpsc::Receiver<DongleTaskMessage>,
        mut shutdown: oneshot::Receiver<()>,
    ) {
        let (commands, commands_lifetime) = commands::dispatch(self.hub.clone(), commands);
        let initial_devices = self.request_initial_devices().await;
        let mut events = self.event_stream(initial_devices, commands);

        loop {
            let event = tokio::select! {
                biased;
                _ = &mut shutdown => break,
                event = events.next() => match event {
                    Some(event) => event,
                    None => break,
                },
            };
            let flow = match event {
                HubEvent::Snapshot(snapshot) => self.on_snapshot_received(snapshot).await,
                HubEvent::Retry => {
                    self.on_retry_tick().await;
                    ControlFlow::Continue(())
                }
                HubEvent::Command(command) => {
                    self.on_dongle_command(command).await;
                    ControlFlow::Continue(())
                }
            };
            if flow.is_break() {
                break;
            }
        }

        drop(commands_lifetime);
        self.shut_down().await;
    }

    /// Request the initial device list, with retries.
    async fn request_initial_devices(&self) -> Option<DeviceSnapshot> {
        let mut attempt = 0u8;
        match (|| self.hub.request_devices())
            .retry(
                ConstantBuilder::default()
                    .with_delay(INITIAL_DEVICE_REQUEST_RETRY_DELAY)
                    .with_max_times(usize::from(INITIAL_DEVICE_REQUEST_ATTEMPTS - 1)),
            )
            .notify(|e, _delay| {
                attempt += 1;
                debug!("Failed to request devices (attempt {attempt}): {e}, retrying...");
            })
            .await
        {
            Ok(list) => {
                if !list.is_empty() {
                    tracing::info!("Hub manager found {} devices on startup", list.len());
                }
                Some(list)
            }
            Err(e) => {
                warn!(
                    "Failed to request devices after {INITIAL_DEVICE_REQUEST_ATTEMPTS} attempts: {e}"
                );
                None
            }
        }
    }

    /// Merge the hub's device snapshots, the retry ticks and the dongle's commands into one
    /// tagged stream under one supervising loop.
    fn event_stream(
        &self,
        initial_devices: Option<DeviceSnapshot>,
        commands: mpsc::Receiver<ControlCommand>,
    ) -> BoxStream<'static, HubEvent> {
        // The dongle sends a snapshot whenever its device list changes.
        info!("Creating snapshot stream for hub");
        let snapshots = stream::unfold(self.hub.clone(), |hub| async move {
            debug!("Waiting for device snapshot from hub...");
            let snapshot = hub.receive_snapshot().await;
            match &snapshot {
                Ok(devices) => {
                    info!(
                        "receive_snapshot returned Ok with {} devices",
                        devices.len()
                    );
                }
                Err(e) => error!("receive_snapshot returned Err: {:?}", e),
            }
            Some((HubEvent::Snapshot(snapshot), hub))
        });
        let snapshots = match initial_devices {
            Some(devices) => {
                info!(
                    "Prepending {} initial devices to snapshot stream",
                    devices.len()
                );
                stream::once(async move { HubEvent::Snapshot(Ok(devices)) })
                    .chain(snapshots)
                    .boxed()
            }
            None => snapshots.boxed(),
        };

        // Delay: a blocked connect attempt must not leave a burst of overdue ticks that
        // would fire retries back to back.
        let mut retry_interval = interval(RETRY_TICK_INTERVAL);
        retry_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let retry_ticks = stream::unfold(retry_interval, |mut interval| async move {
            interval.tick().await;
            Some((HubEvent::Retry, interval))
        });

        let command_events = ReceiverStream::new(commands).map(HubEvent::Command);
        (snapshots, retry_ticks, command_events).merge().boxed()
    }

    /// A tick only matters while some device failed to connect and still has attempts
    /// left. Then re-run the last snapshot so the device is retried even though the
    /// dongle sends no new snapshot.
    async fn on_retry_tick(&mut self) {
        if !self
            .failed_adds
            .values()
            .any(|attempts| *attempts < MAX_HUB_CONNECT_ATTEMPTS)
        {
            return;
        }
        let snapshot = self.last_snapshot.clone();
        self.on_snapshot(snapshot, true).await;
    }

    /// `Break` means the hub is gone and the manager should end.
    async fn on_snapshot_received(&mut self, snapshot: Result<DeviceSnapshot>) -> ControlFlow<()> {
        match snapshot {
            Ok(devices) => {
                self.on_snapshot(devices, false).await;
                ControlFlow::Continue(())
            }
            Err(e) => {
                tracing::error!(
                    "Hub manager receive error - hub likely disconnected: {:?}",
                    e
                );
                // Any receive error, a closed channel included, ends the manager.
                ControlFlow::Break(())
            }
        }
    }

    async fn on_dongle_command(&mut self, command: ControlCommand) {
        match command {
            ControlCommand::StartPairing { timeout_ms, reply } => {
                let result = self
                    .hub
                    .start_pairing(timeout_ms)
                    .await
                    .map_err(|e| e.to_string());
                if result.is_ok() {
                    self.spawn_pairing_result_watcher();
                }
                let _ = reply.send(result);
            }
            ControlCommand::CancelPairing { reply } => {
                let result = self.hub.cancel_pairing().await.map_err(|e| e.to_string());
                let _ = reply.send(result);
            }
            ControlCommand::ClearBonds { reply } => {
                let result = self.hub.clear_bonds().await.map_err(|e| e.to_string());
                if result.is_ok() {
                    self.context
                        .send_message(Message::DongleUpdate(
                            self.dongle.info(self.connected_addresses(), Vec::new()),
                        ))
                        .await;
                }
                let _ = reply.send(result);
            }
        }
    }

    /// Report the outcome of a pairing attempt as a `DonglePairingResult` event once the
    /// dongle settles it. Detached on purpose: it only waits for one answer from the
    /// dongle and broadcasts it, and it ends on its own when the hub disconnects.
    fn spawn_pairing_result_watcher(&self) {
        let hub = self.hub.clone();
        let context = self.context.clone();
        let dongle_id = self.dongle.id.clone();
        tokio::spawn(async move {
            let (success, paired_address, paired_name, error) = match hub.wait_pairing_event().await
            {
                Ok(PairingEvent::Result(device)) => {
                    (true, device.uuid, device.name.to_string(), String::new())
                }
                Ok(PairingEvent::Timeout) => {
                    (false, [0; 6], String::new(), "Pairing timed out".into())
                }
                Ok(PairingEvent::Cancelled) => {
                    (false, [0; 6], String::new(), "Pairing cancelled".into())
                }
                Err(e) => (false, [0; 6], String::new(), e.to_string()),
            };
            if success && !paired_name.is_empty() {
                tracing::info!(
                    "pairing: storing name {:?} for addr={:02x?}",
                    paired_name,
                    paired_address
                );
                context
                    .bond_names
                    .lock()
                    .await
                    .insert(paired_address, paired_name.clone());
            }
            let _ = context.event_sender.send(Event::DonglePairingResult {
                dongle_id,
                success,
                paired_address,
                paired_name,
                error,
            });
        });
    }

    /// Unregister the dongle and take its Bluetooth events away from the units it served.
    async fn shut_down(self) {
        self.context
            .send_message(Message::DongleDisconnect(self.dongle.id.clone()))
            .await;
        tracing::info!("Unregistered dongle '{}' from dongle list", self.dongle.id);

        tracing::info!(
            "Hub manager task ending for hub {:?}, cleaning up {} devices",
            self.serial,
            self.managed_devices.len()
        );
        for (address, device) in &self.managed_devices {
            // Target the handler that accepted this hub's events, not a replacement
            // that happens to have the same UUID after a reboot.
            tracing::info!(
                "Sending RemoveEventsDevice for addr {:02x?} (UUID {:02x?})",
                address,
                device.uuid
            );
            let _ = device
                .commands
                .send(DeviceTaskMessage::RemoveEventsDevice(Arc::clone(
                    &self.events_owner,
                )))
                .await;
        }
        tracing::info!("Hub manager task ended for hub {}", self.serial);
    }

    fn connected_addresses(&self) -> Vec<DeviceAddress> {
        self.managed_devices.keys().copied().collect()
    }
}
