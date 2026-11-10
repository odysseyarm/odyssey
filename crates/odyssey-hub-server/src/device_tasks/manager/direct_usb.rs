//! Units plugged in over USB (as opposed to reached through a dongle): probe, connect,
//! hand to a [`DeviceHandler`], and tell it when the cable goes away.

use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};
use odyssey_protocol::ProductId;
use odyssey_protocol::control::device::TransportMode;
use odyssey_usb::device::VmDevice;
use tokio::sync::mpsc;
use tracing::{debug, info};

use super::{PresentUsbDevices, UsbDeviceManager};
use crate::device_tasks::context::{DeviceAddress, RunningDevice};
use crate::device_tasks::handler::{DeviceConnections, DeviceHandler, DeviceHandlerInit};
use crate::device_tasks::messages::{DeviceChannels, DeviceTaskMessage, Message};
use crate::device_tasks::netcore_probe::{NetCoreLink, spawn_netcore_probe};
use crate::device_tasks::transport_selection::TransportSelection;
use crate::device_tasks::usb_probe::{
    read_events_protocol_version, read_firmware_version, read_transport_mode, read_uuid_with_retry,
};
use crate::firmware::DFU_CLASS;

/// Capacity of a handler's control channel.
const HANDLER_CHANNEL_CAPACITY: usize = 32;

/// Discovery metadata independent of the USB enumerator.
pub(super) struct DirectUsbMetadata<'a> {
    pub(super) product_id: u16,
    pub(super) product_name: &'a str,
    pub(super) dfu: bool,
}

impl<'a> DirectUsbMetadata<'a> {
    fn from_usb(info: &'a nusb::DeviceInfo) -> Self {
        Self {
            product_id: info.product_id(),
            product_name: info.product_string().unwrap_or_default(),
            dfu: info
                .interfaces()
                .any(|interface| interface.class() == DFU_CLASS),
        }
    }
}

/// A unit that answered on its USB control or packet endpoint, with what it reported.
struct ProbedUsbDevice {
    uuid: DeviceAddress,
    firmware: Option<odyssey_protocol::Version>,
    events_protocol_version: [u16; 3],
    transport_mode: TransportMode,
    transport_selection: TransportSelection,
    dfu: bool,
    vm_device: VmDevice,
}

impl ProbedUsbDevice {
    /// Control protocol version, which says whether the unit can report its net core release.
    fn control_protocol_version(&self) -> Option<[u16; 3]> {
        self.firmware
            .as_ref()
            .map(|version| version.protocol_semver)
    }

    /// Firmware that picks its own transport streams over USB whenever a host has the
    /// cable configured, which the probe itself proves.
    fn streams_over_usb(&self) -> bool {
        self.transport_selection == TransportSelection::Automatic
            || self.transport_mode == TransportMode::Usb
    }
}

/// How an already-running handler relates to a unit that was just found on USB.
enum ExistingHandler {
    None,
    /// Created for a direct-USB connection; its control device is already embedded.
    DirectUsb(mpsc::Sender<DeviceTaskMessage>),
    /// Created for a BLE-mux connection; it can take the USB control device.
    BleMux(mpsc::Sender<DeviceTaskMessage>),
}

impl UsbDeviceManager {
    pub(super) async fn on_direct_usb_device(
        &mut self,
        dev_info: &nusb::DeviceInfo,
        product_id: ProductId,
        present: &mut PresentUsbDevices,
    ) {
        let direct_usb_id = format!("{:?}", dev_info.id());
        present.direct_usb_ids.insert(direct_usb_id.clone());
        debug!("Found direct USB device {product_id:?}, checking if already connected");
        if self.handler_holds_usb(&direct_usb_id).await {
            return;
        }

        let Some(probed) = self
            .probe_direct_usb_device(dev_info, product_id, &direct_usb_id, present)
            .await
        else {
            return;
        };
        self.register_direct_usb(direct_usb_id, DirectUsbMetadata::from_usb(dev_info), probed)
            .await;
    }

    /// True while a running handler owns this USB device's connection. Probing it then
    /// can only fail with EBUSY (which nusb logs as an error), and a successful probe
    /// would be dropped by `hand_to_existing_handler` anyway.
    async fn handler_holds_usb(&self, direct_usb_id: &str) -> bool {
        let Some(&uuid) = self.direct_usb_id_to_uuid.get(direct_usb_id) else {
            return false;
        };
        let running = self
            .device_handles
            .lock()
            .await
            .get(&uuid)
            .is_some_and(|device| !device.task.is_finished());
        running
            && self
                .context
                .direct_usb_handler_uuids
                .lock()
                .await
                .contains(&uuid)
    }

    #[cfg(feature = "sim")]
    pub(super) async fn on_simulated_unit(&mut self, connection: VmDevice, id: String) {
        let Ok(odyssey_protocol::Props::ProductId(product_id)) = connection
            .read_prop(odyssey_protocol::PropKind::ProductId)
            .await
        else {
            return;
        };
        let Some(product_id) = ProductId::from_u16(product_id) else {
            return;
        };
        if let Some(probed) = Self::probe_connection(connection, product_id, false).await {
            self.register_direct_usb(
                id,
                DirectUsbMetadata {
                    product_id: product_id as u16,
                    product_name: "Odyssey Simulator",
                    dfu: false,
                },
                probed,
            )
            .await;
        }
    }

    async fn register_direct_usb(
        &mut self,
        direct_usb_id: String,
        metadata: DirectUsbMetadata<'_>,
        probed: ProbedUsbDevice,
    ) {
        let uuid = probed.uuid;
        let pid = metadata.product_id;
        let replace_control = self.direct_usb_id_to_uuid.get(&direct_usb_id) != Some(&uuid);
        self.direct_usb_id_to_uuid.insert(direct_usb_id, uuid);
        self.last_uuid_by_pid.insert(pid, uuid);

        if self
            .hand_to_existing_handler(&probed, replace_control)
            .await
        {
            // Dropping `probed` closes the connection that was only opened to find that out.
            return;
        }

        // No handler yet, or a BLE-mux handler is being replaced; the probe's connection
        // is reused.
        info!("Connecting new USB device with UUID {uuid:02x?}, PID: 0x{pid:04x}");
        self.spawn_direct_usb_handler(&metadata, probed).await;
    }

    /// Connect and read UUID/version/transport mode. The connection is kept if the unit
    /// turns out to need a handler.
    async fn probe_direct_usb_device(
        &mut self,
        dev_info: &nusb::DeviceInfo,
        product_id: ProductId,
        direct_usb_id: &str,
        present: &mut PresentUsbDevices,
    ) -> Option<ProbedUsbDevice> {
        let vm_device = match VmDevice::connect_usb_any_mode(dev_info.clone()).await {
            Ok(vm_device) => vm_device,
            Err(e) => {
                self.on_unconnectable_usb_device(dev_info, direct_usb_id, &e, present)
                    .await;
                return None;
            }
        };

        Self::probe_connection(
            vm_device,
            product_id,
            DirectUsbMetadata::from_usb(dev_info).dfu,
        )
        .await
    }

    async fn probe_connection(
        vm_device: VmDevice,
        product_id: ProductId,
        dfu: bool,
    ) -> Option<ProbedUsbDevice> {
        // Control plane for AtsLite/AtsPro (control endpoint), packet protocol for AtsVm.
        let uuid = read_uuid_with_retry(&vm_device, product_id).await?;
        let firmware = read_firmware_version(&vm_device, uuid).await;
        let transport_selection = firmware
            .as_ref()
            .map_or(TransportSelection::Manual, |version| {
                TransportSelection::for_protocol(version.protocol_semver)
            });
        let transport_mode = read_transport_mode(&vm_device, uuid).await;
        // Only available when the unit is in USB transport mode (direct events).
        let events_protocol_version = if transport_selection == TransportSelection::Automatic
            || transport_mode == TransportMode::Usb
        {
            read_events_protocol_version(&vm_device, uuid).await
        } else {
            [0, 0, 0]
        };
        Some(ProbedUsbDevice {
            uuid,
            firmware,
            events_protocol_version,
            transport_mode,
            transport_selection,
            dfu,
            vm_device,
        })
    }

    /// Hand the USB control device to a handler that already exists for the unit, if it can
    /// use it. Returns `true` when a handler exists (so the caller must not create another).
    async fn hand_to_existing_handler(
        &self,
        probed: &ProbedUsbDevice,
        replace_control: bool,
    ) -> bool {
        let uuid = probed.uuid;
        match self.existing_handler(uuid).await {
            ExistingHandler::None => {
                info!("Device {uuid:02x?} not in handles, will connect");
                false
            }
            ExistingHandler::DirectUsb(_) if !replace_control => {
                // Sending AddControlDevice to an established direct-USB handler on every
                // scan would re-read the transport mode on stale VmDevice instances and
                // corrupt the handler's state.
                info!(
                    "Device {uuid:02x?} already has direct-USB handler, skipping AddControlDevice"
                );
                true
            }
            ExistingHandler::BleMux(commands) | ExistingHandler::DirectUsb(commands) => {
                // A BLE-mux handler, or a direct-USB one being replaced, takes the USB control.
                info!(
                    "Device {uuid:02x?} has BLE-mux handler, sending AddControlDevice to merge USB control"
                );
                let result = commands
                    .send(DeviceTaskMessage::AddControlDevice {
                        device: probed.vm_device.clone(),
                        dfu: probed.dfu,
                        events_protocol_version: probed.events_protocol_version,
                        transport_selection: probed.transport_selection,
                    })
                    .await;
                if result.is_ok() {
                    self.context
                        .direct_usb_handler_uuids
                        .lock()
                        .await
                        .insert(uuid);
                }
                spawn_netcore_probe(
                    NetCoreLink::Control(probed.vm_device.clone()),
                    probed.control_protocol_version(),
                    commands,
                );
                info!("AddControlDevice message sent for {uuid:02x?}, result: {result:?}");
                true
            }
        }
    }

    async fn existing_handler(&self, uuid: DeviceAddress) -> ExistingHandler {
        let mut handles = self.device_handles.lock().await;
        info!(
            "Checking if device {uuid:02x?} is already connected (current handles: {})",
            handles.len()
        );

        // SetTransportMode ends its handler at once, which can leave a finished entry
        // in device_handles; without evicting it the next scan would never reconnect
        // the device.
        if handles
            .get(&uuid)
            .is_some_and(|device| device.task.is_finished())
        {
            info!("Device {uuid:02x?} has a finished handler, evicting stale entry");
            handles.remove(&uuid);
            self.context
                .direct_usb_handler_uuids
                .lock()
                .await
                .remove(&uuid);
        }

        let Some(existing) = handles.get(&uuid) else {
            return ExistingHandler::None;
        };
        let commands = existing.commands.clone();
        drop(handles);
        if self
            .context
            .direct_usb_handler_uuids
            .lock()
            .await
            .contains(&uuid)
        {
            ExistingHandler::DirectUsb(commands)
        } else {
            ExistingHandler::BleMux(commands)
        }
    }

    async fn spawn_direct_usb_handler(
        &self,
        metadata: &DirectUsbMetadata<'_>,
        probed: ProbedUsbDevice,
    ) {
        // USB scans and dongle snapshots can discover the same UUID concurrently.
        // Check and install under one lock so neither task replaces a live handler.
        let mut handles = self.device_handles.lock().await;
        if handles
            .get(&probed.uuid)
            .is_some_and(|device| !device.task.is_finished())
        {
            drop(handles);
            self.hand_to_existing_handler(&probed, true).await;
            return;
        }
        let uuid = probed.uuid;
        let device = direct_usb_device(metadata, &probed);
        let (commands, control_rx) = mpsc::channel(HANDLER_CHANNEL_CAPACITY);
        let netcore_probe_link = NetCoreLink::Control(probed.vm_device.clone());
        let control_protocol_version = probed.control_protocol_version();

        // A unit in USB transport mode streams events over the same connection it is
        // controlled over; in BLE mode only control comes over USB.
        let connections = if probed.streams_over_usb() {
            DeviceConnections {
                control_device: Some(probed.vm_device.clone()),
                events_device: Some(probed.vm_device),
            }
        } else {
            DeviceConnections {
                control_device: Some(probed.vm_device),
                events_device: None,
            }
        };
        let task = DeviceHandler::spawn(DeviceHandlerInit {
            connections,
            device: device.clone(),
            control_rx,
            context: self.context.clone(),
            protocol_version: probed.events_protocol_version,
            hub_device: None,
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
            .insert(uuid);
        drop(handles);
        let netcore_probe_commands = commands.clone();

        // The channel always goes to the device list so commands have a live sender;
        // the handler serves those that need events (like Zero) once events are available.
        let channels = DeviceChannels {
            commands: Some(commands),
        };
        self.context
            .send_message(Message::Connect(device, channels))
            .await;

        spawn_netcore_probe(
            netcore_probe_link,
            control_protocol_version,
            netcore_probe_commands,
        );
    }

    /// Reconcile direct USB disconnects by opaque `id()`, so a scan does not depend on
    /// re-probing every UUID successfully.
    pub(super) async fn reconcile_direct_usb_disconnects(&mut self, present: &PresentUsbDevices) {
        let missing_ids: Vec<String> = self
            .direct_usb_id_to_uuid
            .keys()
            .filter(|id| !present.direct_usb_ids.contains(*id))
            .cloned()
            .collect();
        if missing_ids.is_empty() {
            return;
        }

        for id in missing_ids {
            let Some(uuid) = self.direct_usb_id_to_uuid.remove(&id) else {
                continue;
            };
            // A unit that re-enumerates (a reboot, a transport switch) shows up
            // at a new address while its old one is still in the map. The
            // handler for that UUID is then the new connection's, so telling it
            // its USB control left would tear it down seconds after it started.
            let reappeared = self
                .direct_usb_id_to_uuid
                .iter()
                .any(|(other, other_uuid)| {
                    *other_uuid == uuid && present.direct_usb_ids.contains(other)
                });
            if reappeared {
                info!(
                    "Direct USB address {} of {:02x?} is gone but the unit is present at another address",
                    id, uuid
                );
                continue;
            }
            let commands = self
                .device_handles
                .lock()
                .await
                .get(&uuid)
                .map(|device| device.commands.clone());
            let Some(commands) = commands else {
                info!(
                    "Direct USB device missing from scan but no active handler: id={} uuid={:02x?}",
                    id, uuid
                );
                continue;
            };
            info!(
                "Direct USB device missing from scan, sending RemoveControlDevice: id={} uuid={:02x?}",
                id, uuid
            );
            let _ = commands.send(DeviceTaskMessage::RemoveControlDevice).await;
        }
    }
}

/// The device-list entry for a unit that is plugged in over USB.
fn direct_usb_device(metadata: &DirectUsbMetadata<'_>, probed: &ProbedUsbDevice) -> Device {
    let in_usb_mode = probed.streams_over_usb();

    // CONTROL always; EVENTS only in USB transport mode; DFU whenever the device exposes
    // a DFU interface (class 0xFE).
    let mut capabilities = DeviceCapabilities::new(DeviceCapabilities::CONTROL);
    if in_usb_mode {
        capabilities.insert(DeviceCapabilities::EVENTS);
    }
    if probed.transport_selection == TransportSelection::Automatic {
        capabilities.insert(DeviceCapabilities::AUTO_TRANSPORT);
    }
    if metadata.dfu {
        capabilities.insert(DeviceCapabilities::DFU);
    }

    Device {
        uuid: probed.uuid,
        transport: Transport::Usb,
        capabilities,
        firmware_version: probed
            .firmware
            .as_ref()
            .map_or([0, 0, 0], |version| version.firmware_semver),
        events_transport: if in_usb_mode {
            EventsTransport::Wired
        } else {
            EventsTransport::Bluetooth
        },
        events_connected: in_usb_mode,
        product_id: metadata.product_id,
        name: Device::name_bytes(metadata.product_name),
        netcore_release: 0,
    }
}

#[cfg(all(test, feature = "sim"))]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use arc_swap::ArcSwap;
    use odyssey_sim::{Command, SensorModel, Simulator};
    use tokio::sync::{Mutex, RwLock, broadcast};

    use super::*;
    use crate::device_tasks::context::DeviceTaskContext;

    #[tokio::test]
    async fn simulated_registration_distinguishes_modern_from_reboot_required_cameras() {
        for (model, expected) in [
            (SensorModel::Pag7665qn, ProductId::AtsPro),
            (SensorModel::Paj7025, ProductId::AtsLite),
        ] {
            let simulator = Simulator::with_sensor_model(model);
            simulator
                .handle()
                .execute(Command::UnitUsb { plugged: true })
                .await
                .unwrap();
            let (message_channel, mut messages) = mpsc::channel(16);
            let context = DeviceTaskContext {
                message_channel,
                screen_calibrations: Arc::new(ArcSwap::from_pointee(Default::default())),
                device_offsets: Arc::new(Mutex::new(Default::default())),
                device_shot_delays: Arc::new(RwLock::new(Default::default())),
                shot_delay_watch: Arc::new(RwLock::new(Default::default())),
                event_sender: broadcast::channel(16).0,
                direct_usb_handler_uuids: Arc::new(Mutex::new(Default::default())),
                bond_names: Arc::new(Mutex::new(Default::default())),
            };
            let mut manager = UsbDeviceManager::new(context);
            tokio::time::timeout(
                Duration::from_secs(5),
                manager.on_simulated_unit(
                    simulator.connect_unit_usb(odyssey_sim::UNIT_UUID),
                    "sim:test".to_owned(),
                ),
            )
            .await
            .unwrap();
            let Some(Message::Connect(device, _)) =
                tokio::time::timeout(Duration::from_secs(5), messages.recv())
                    .await
                    .unwrap()
            else {
                panic!("simulated unit must register a connected device");
            };
            assert_eq!(device.product_id, expected as u16, "{model:?}");
            drop(manager);
            simulator.shutdown().await.unwrap();
        }
    }
}
