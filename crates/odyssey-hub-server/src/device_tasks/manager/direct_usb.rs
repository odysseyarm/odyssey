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
use crate::device_tasks::usb_probe::{
    read_events_protocol_version, read_firmware_version, read_transport_mode, read_uuid_with_retry,
};
use crate::firmware::DFU_CLASS;

/// Capacity of a handler's control channel.
const HANDLER_CHANNEL_CAPACITY: usize = 32;

/// A unit that answered on its USB control or packet endpoint, with what it told us.
struct ProbedUsbDevice {
    uuid: DeviceAddress,
    firmware: Option<odyssey_protocol::Version>,
    events_protocol_version: [u16; 3],
    transport_mode: TransportMode,
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
        let pid = dev_info.product_id();
        let direct_usb_id = format!("{:?}", dev_info.id());
        present.direct_usb_ids.insert(direct_usb_id.clone());
        debug!("Found direct USB device {product_id:?}, checking if already connected");

        let Some(probed) = self
            .probe_direct_usb_device(dev_info, product_id, &direct_usb_id, present)
            .await
        else {
            return;
        };
        let uuid = probed.uuid;

        let replace_control = self.direct_usb_id_to_uuid.get(&direct_usb_id) != Some(&uuid);
        // Cache mapping from opaque nusb id() to UUID once we've successfully probed.
        self.direct_usb_id_to_uuid.insert(direct_usb_id, uuid);
        self.last_uuid_by_pid.insert(pid, uuid);

        // Now check if device is already connected BEFORE creating a new connection
        if self
            .hand_to_existing_handler(&probed, replace_control)
            .await
        {
            // Dropping `probed` drops the connection we only made to find that out.
            return;
        }

        // Device is not connected or we're replacing BLE mux connection
        // Reuse the connection we already created
        info!("Connecting new USB device with UUID {uuid:02x?}, PID: 0x{pid:04x}");
        self.spawn_direct_usb_handler(dev_info, probed).await;
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

        // Control plane for AtsLite/AtsPro (control endpoint), packet protocol for AtsVm.
        let uuid = read_uuid_with_retry(&vm_device, product_id).await?;
        let firmware = read_firmware_version(&vm_device, uuid).await;
        let transport_mode = read_transport_mode(&vm_device, uuid).await;
        // Only available when the unit is in USB transport mode (direct events).
        let events_protocol_version = if transport_mode == TransportMode::Usb {
            read_events_protocol_version(&vm_device, uuid).await
        } else {
            [0, 0, 0]
        };
        Some(ProbedUsbDevice {
            uuid,
            firmware,
            events_protocol_version,
            transport_mode,
            dfu: dev_info
                .interfaces()
                .any(|interface| interface.class() == DFU_CLASS),
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
                // Already a fully-established direct-USB handler — no merge needed.
                // Sending AddControlDevice every scan cycle would re-read transport
                // mode on stale VmDevice instances and corrupt the handler's state.
                info!(
                    "Device {uuid:02x?} already has direct-USB handler, skipping AddControlDevice"
                );
                true
            }
            ExistingHandler::BleMux(commands) | ExistingHandler::DirectUsb(commands) => {
                // BLE-mux primary handler — merge incoming USB control.
                info!(
                    "Device {uuid:02x?} has BLE-mux handler, sending AddControlDevice to merge USB control"
                );
                let result = commands
                    .send(DeviceTaskMessage::AddControlDevice {
                        device: probed.vm_device.clone(),
                        dfu: probed.dfu,
                        events_protocol_version: probed.events_protocol_version,
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

        // Evict stale entries whose handler task has already finished.
        // This can happen after SetTransportMode causes the handler to break
        // immediately, leaving a dead entry in device_handles. Without this
        // eviction the next scan pass would skip re-connecting the device.
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

    async fn spawn_direct_usb_handler(&self, dev_info: &nusb::DeviceInfo, probed: ProbedUsbDevice) {
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
        let device = direct_usb_device(dev_info, &probed);
        let (commands, control_rx) = mpsc::channel(HANDLER_CHANNEL_CAPACITY);
        let netcore_probe_link = NetCoreLink::Control(probed.vm_device.clone());
        let control_protocol_version = probed.control_protocol_version();

        // A unit in USB transport mode streams events over the same connection it is
        // controlled over; in BLE mode only control comes over USB.
        let connections = if probed.transport_mode == TransportMode::Usb {
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
            hub_device: None, // USB-direct device: no hub to update
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

        // Always pass the commands channel so server.device_list gets a working sender.
        // The device handler task will process commands (like Zero) once events are available.
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

    /// Reconcile direct USB disconnects using opaque id() keys.
    /// This avoids requiring UUID re-probe success on every scan.
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
fn direct_usb_device(dev_info: &nusb::DeviceInfo, probed: &ProbedUsbDevice) -> Device {
    let in_usb_mode = probed.transport_mode == TransportMode::Usb;

    // USB direct always has CONTROL capability
    // EVENTS capability depends on transport mode (USB mode has events, BLE mode doesn't)
    // DFU capability is present whenever the device exposes a DFU interface (class 0xFE)
    let mut capabilities = DeviceCapabilities::new(DeviceCapabilities::CONTROL);
    if in_usb_mode {
        capabilities.insert(DeviceCapabilities::EVENTS);
    }
    if dev_info
        .interfaces()
        .any(|interface| interface.class() == DFU_CLASS)
    {
        capabilities.insert(DeviceCapabilities::DFU);
    }

    let product_name = dev_info
        .product_string()
        .filter(|name| !name.is_empty())
        .unwrap_or_default();
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
        product_id: dev_info.product_id(),
        name: Device::name_bytes(product_name),
        netcore_release: 0,
    }
}
