//! Reacting to the dongle's list of connected units: connect to new ones, release gone ones.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};
use odyssey_protocol::{PropKind, Props, Version};
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::PacketData;
use tokio::sync::mpsc;

use super::{DeviceSnapshot, HubManager, MAX_HUB_CONNECT_ATTEMPTS, ManagedDevice};
use crate::device_tasks::context::{DeviceAddress, RunningDevice};
use crate::device_tasks::handler::{DeviceConnections, DeviceHandler, DeviceHandlerInit};
use crate::device_tasks::messages::{DeviceChannels, DeviceTaskMessage, Message};
use crate::device_tasks::transport_selection::TransportSelection;

/// Capacity of a handler's control channel.
const HANDLER_CHANNEL_CAPACITY: usize = 32;

/// The version read is the first packet a fresh BLE link carries, and the first packet
/// after a unit (re)boots is sometimes lost, so it is re-sent.
const VERSION_READ_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_READ_ATTEMPTS: usize = 3;

/// What a unit reached over Bluetooth says about itself.
struct BleDeviceIdentity {
    uuid: DeviceAddress,
    firmware_version: [u16; 3],
    /// Events protocol version, read via the packet interface (works over BLE).
    events_protocol_version: [u16; 3],
    product_id: u16,
    name: [u8; 33],
}

impl HubManager {
    /// Bring the managed units in line with a snapshot. On a retry (`is_retry`) units that
    /// already used up their connect attempts are skipped.
    pub(super) async fn on_snapshot(&mut self, snapshot: DeviceSnapshot, is_retry: bool) {
        tracing::info!(
            "Hub received device snapshot with {} devices: {:02x?}",
            snapshot.len(),
            snapshot
        );
        self.last_snapshot = snapshot.clone();
        if !is_retry {
            // A fresh snapshot is a fresh chance for every device.
            self.failed_adds.clear();
        }
        let new_set: HashSet<DeviceAddress> = snapshot.into_iter().collect();
        // A transport switch can end a handler before the dongle reports a new
        // address. A fresh snapshot must be able to attach a replacement handler.
        self.managed_devices
            .retain(|_, device| !device.commands.is_closed());
        self.failed_adds
            .retain(|address, _| new_set.contains(address));
        // Managed devices are keyed by BLE address; device_handles is keyed by UUID.
        let existing_set: HashSet<DeviceAddress> = self.managed_devices.keys().copied().collect();

        let added: Vec<DeviceAddress> = new_set.difference(&existing_set).copied().collect();
        tracing::info!(
            "Device diff - new: {:02x?}, existing: {:02x?}, added: {:02x?}, removed: {:02x?}",
            new_set,
            existing_set,
            added,
            existing_set
                .difference(&new_set)
                .copied()
                .collect::<Vec<_>>()
        );

        // Bond names are refreshed first so new devices pick up the dongle's names.
        if !added.is_empty()
            && self.dongle.supports_list_bonds()
            && let Ok(bonds) = self.hub.list_bonds().await
        {
            self.remember_missing_bond_names(&bonds).await;
        }

        for address in added {
            if is_retry && self.failed_attempts(address) >= MAX_HUB_CONNECT_ATTEMPTS {
                continue;
            }
            tracing::info!("DevicesSnapshot: Adding new device {address:02x?}");
            self.add_device(address).await;
        }
        self.release_departed_devices(&new_set).await;

        // Dongle list subscribers see the changed set of connected devices.
        let bonded_devices = self.refresh_bonded_devices().await;
        self.context
            .send_message(Message::DongleUpdate(
                self.dongle.info(self.connected_addresses(), bonded_devices),
            ))
            .await;
    }

    fn failed_attempts(&self, address: DeviceAddress) -> u8 {
        self.failed_adds.get(&address).copied().unwrap_or(0)
    }

    /// Connect to a newly reported unit and give it a handler, or attach the dongle's events
    /// to the handler a cable already created for it.
    async fn add_device(&mut self, address: DeviceAddress) {
        let Some((vm_device, version)) = self.connect_to_device(address).await else {
            return;
        };
        let identity = self
            .read_device_identity(&vm_device, address, version)
            .await;
        let uuid = identity.uuid;
        let transport_selection =
            TransportSelection::for_protocol(identity.events_protocol_version);
        let mut capabilities = DeviceCapabilities::empty();
        if transport_selection == TransportSelection::Automatic {
            capabilities.insert(DeviceCapabilities::AUTO_TRANSPORT);
        }
        let device = Device {
            uuid,
            transport: Transport::UsbMux,
            // AddEventsDevice sets EVENTS and events_connected once the streams are
            // established.
            capabilities,
            firmware_version: identity.firmware_version,
            events_transport: EventsTransport::Bluetooth,
            events_connected: false,
            product_id: identity.product_id,
            name: identity.name,
            netcore_release: 0,
        };

        // A unit already connected over USB gets the BLE events merged into its handler.
        let commands = match self.existing_handler_commands(uuid).await {
            Some(commands) => {
                tracing::info!(
                    "Device {uuid:02x?} found in device_handles, merging by adding BLE mux for events"
                );
                let result = commands
                    .send(DeviceTaskMessage::AddEventsDevice {
                        device: vm_device,
                        protocol_version: identity.events_protocol_version,
                        hub: self.hub.clone(),
                        address,
                        owner: Arc::clone(&self.events_owner),
                    })
                    .await;
                tracing::info!("AddEventsDevice message sent for {uuid:02x?}, result: {result:?}");
                if result.is_err() {
                    return;
                }
                commands
            }
            None => {
                tracing::info!(
                    "Device {uuid:02x?} not found in device_handles, creating new BLE mux handler"
                );
                self.spawn_ble_handler(device, vm_device, identity.events_protocol_version, address)
                    .await
            }
        };
        // Tracked by BLE address so it gets cleaned up when the hub disconnects.
        self.managed_devices
            .insert(address, ManagedDevice { uuid, commands });
    }

    /// Open a link to the unit and read its version, which says whether its streams have
    /// to be reset. A failure is counted so the retry tick gives up after
    /// [`MAX_HUB_CONNECT_ATTEMPTS`].
    ///
    /// Old firmware keeps a separate set of streams per link, and the ones a previous
    /// session left on this link are stopped before anything else is read. Firmware that
    /// picks its own transport shares one set between USB and BLE, so a reset here would
    /// cut off the reports a plugged-in unit is sending over USB.
    async fn connect_to_device(&mut self, address: DeviceAddress) -> Option<(VmDevice, Version)> {
        let vm_device = VmDevice::open_via_mux(self.hub.clone(), address);
        let connected = async {
            let version = read_version(&vm_device).await?;
            if TransportSelection::for_protocol(version.protocol_semver)
                == TransportSelection::Manual
            {
                vm_device.clear_all_streams().await?;
            }
            anyhow::Ok(version)
        };
        match connected.await {
            Ok(version) => {
                self.failed_adds.remove(&address);
                Some((vm_device, version))
            }
            Err(e) => {
                let attempts = self.failed_adds.entry(address).or_insert(0);
                *attempts += 1;
                tracing::warn!(
                    "Failed to connect to hub device {address:02x?} during snapshot (attempt {attempts}/{MAX_HUB_CONNECT_ATTEMPTS}): {e}"
                );
                None
            }
        }
    }

    async fn read_device_identity(
        &self,
        vm_device: &VmDevice,
        address: DeviceAddress,
        version: Version,
    ) -> BleDeviceIdentity {
        let uuid = match vm_device.read_prop(PropKind::Uuid).await {
            Ok(Props::Uuid(uuid_bytes)) => uuid_bytes,
            Ok(other) => {
                tracing::warn!("Expected Uuid prop, got {:?}", other);
                address
            }
            Err(e) => {
                tracing::warn!("Failed to read UUID from device: {}", e);
                address
            }
        };
        tracing::info!(
            "BLE device {address:02x?} has UUID {uuid:02x?}, firmware version: {:?}, events protocol: {:?}",
            version.firmware_semver,
            version.protocol_semver,
        );

        let product_id = match vm_device.read_prop(PropKind::ProductId).await {
            Ok(Props::ProductId(id)) => id,
            _ => 0,
        };

        BleDeviceIdentity {
            uuid,
            firmware_version: version.firmware_semver,
            events_protocol_version: version.protocol_semver,
            product_id,
            name: self.device_name(vm_device, address).await,
        }
    }

    /// Read the name from the unit and push it to the dongle so its bond cache is always
    /// fresh; fall back to the name remembered for the address.
    async fn device_name(&self, vm_device: &VmDevice, address: DeviceAddress) -> [u8; 33] {
        if let Ok(Props::Name(name)) = vm_device.read_prop(PropKind::Name).await
            && !name.is_empty()
        {
            let name_text = name.to_string();
            self.context
                .bond_names
                .lock()
                .await
                .insert(address, name_text.clone());
            if let Err(e) = self.hub.update_bond_name(address, name.into()).await {
                tracing::warn!("Failed to push name to dongle: {e}");
            }
            return Device::name_bytes(&name_text);
        }
        let remembered = self
            .context
            .bond_names
            .lock()
            .await
            .get(&address)
            .cloned()
            .unwrap_or_default();
        Device::name_bytes(&remembered)
    }

    /// The control channel of the handler already running for `uuid`, after evicting a
    /// finished one.
    async fn existing_handler_commands(
        &self,
        uuid: DeviceAddress,
    ) -> Option<mpsc::Sender<DeviceTaskMessage>> {
        let mut handles = self.device_handles.lock().await;
        tracing::info!(
            "BLE mux device {uuid:02x?} discovered (hub path), checking if already connected. device_handles has {} entries",
            handles.len()
        );

        // SetTransportMode ends its handler at once, which can leave a finished
        // entry in device_handles.
        if handles
            .get(&uuid)
            .is_some_and(|device| device.task.is_finished())
        {
            tracing::info!("BLE: Device {uuid:02x?} has a finished handler, evicting stale entry");
            handles.remove(&uuid);
            self.context
                .direct_usb_handler_uuids
                .lock()
                .await
                .remove(&uuid);
        }
        handles.get(&uuid).map(|device| device.commands.clone())
    }

    /// Create a handler for a unit that is only reachable over Bluetooth.
    async fn spawn_ble_handler(
        &self,
        device: Device,
        vm_device: VmDevice,
        events_protocol_version: [u16; 3],
        address: DeviceAddress,
    ) -> mpsc::Sender<DeviceTaskMessage> {
        let uuid = device.uuid;
        let mut handles = self.device_handles.lock().await;
        if let Some(existing) = handles
            .get(&uuid)
            .filter(|device| !device.task.is_finished())
        {
            let commands = existing.commands.clone();
            drop(handles);
            let _ = commands
                .send(DeviceTaskMessage::AddEventsDevice {
                    device: vm_device,
                    protocol_version: events_protocol_version,
                    hub: self.hub.clone(),
                    address,
                    owner: Arc::clone(&self.events_owner),
                })
                .await;
            return commands;
        }
        let (commands, control_rx) = mpsc::channel(HANDLER_CHANNEL_CAPACITY);

        // Start with no events device and queue AddEventsDevice right after, so BLE-only
        // units take the same handler path as a USB unit that gains BLE. Starting with the
        // events device races: startup reads config before streaming and leaves the
        // vm_device stale.
        let task = DeviceHandler::spawn(DeviceHandlerInit {
            connections: DeviceConnections {
                control_device: None,
                events_device: None,
            },
            device: device.clone(),
            control_rx,
            context: self.context.clone(),
            protocol_version: events_protocol_version,
            hub_device: Some((self.hub.clone(), address)),
            self_tx: commands.downgrade(),
        });
        handles.insert(
            uuid,
            RunningDevice {
                commands: commands.clone(),
                task,
            },
        );
        self.context
            .direct_usb_handler_uuids
            .lock()
            .await
            .remove(&uuid);

        // The name is already set in `device`.
        self.context
            .send_message(Message::Connect(
                device,
                DeviceChannels {
                    commands: Some(commands.clone()),
                },
            ))
            .await;
        drop(handles);

        // The handler starts the streams from AddEventsDevice (start_sensor_streams before
        // read_all_config), as on the USB-first path.
        let _ = commands
            .send(DeviceTaskMessage::AddEventsDevice {
                device: vm_device,
                protocol_version: events_protocol_version,
                hub: self.hub.clone(),
                address,
                owner: Arc::clone(&self.events_owner),
            })
            .await;
        commands
    }

    /// Release the units this hub managed that are gone from the snapshot.
    async fn release_departed_devices(&mut self, still_connected: &HashSet<DeviceAddress>) {
        let departed: Vec<DeviceAddress> = self
            .managed_devices
            .keys()
            .copied()
            .filter(|address| !still_connected.contains(address))
            .collect();
        for address in departed {
            let Some(device) = self.managed_devices.remove(&address) else {
                continue;
            };
            let uuid = device.uuid;
            tracing::info!(
                "DevicesSnapshot: BLE device {address:02x?} (UUID {uuid:02x?}) disconnected from hub"
            );

            // Let the handler decide whether to continue (if it has USB control) or exit;
            // it sends Disconnect itself if it has no remaining connections.
            tracing::info!(
                "Sending RemoveEventsDevice to {uuid:02x?} (may still have USB control)"
            );
            let _ = device
                .commands
                .send(DeviceTaskMessage::RemoveEventsDevice(Arc::clone(
                    &self.events_owner,
                )))
                .await;
        }
    }
}

async fn read_version(vm_device: &VmDevice) -> anyhow::Result<Version> {
    match vm_device
        .request_with_retry(
            PacketData::ReadVersion(),
            VERSION_READ_ATTEMPT_TIMEOUT,
            VERSION_READ_ATTEMPTS,
        )
        .await?
    {
        PacketData::ReadVersionResponse(version) => Ok(version),
        other => anyhow::bail!("unexpected reply to ReadVersion: {other:?}"),
    }
}
