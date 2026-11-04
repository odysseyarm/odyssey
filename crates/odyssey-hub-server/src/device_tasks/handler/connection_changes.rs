//! A unit can be reachable over USB control, over a BLE mux for events, or both, and
//! links come and go while the handler runs. These handle the control messages that
//! attach or detach one, and keep the unit's capabilities and transport in step.

use std::ops::ControlFlow;
use std::sync::Arc;

use odyssey_hub_common::device::{DeviceCapabilities, EventsTransport, Transport};
use odyssey_hub_common::events::DeviceEventKind;
use odyssey_protocol::control::device::TransportMode;
use odyssey_usb::device::{MuxDevice, VmDevice};
use tracing::{info, warn};

use super::{CALL_TIMEOUT, DeviceHandler, SLOW_CALL_TIMEOUT};
use crate::device_tasks::messages::Message;
use crate::device_tasks::netcore_probe::spawn_packet_netcore_probe;
use crate::device_tasks::sensor_streams::SensorStreams;
use crate::device_tasks::tracking::CameraModels;

impl DeviceHandler {
    pub(super) async fn on_add_events_device(
        &mut self,
        events_device: VmDevice,
        events_protocol_version: [u16; 3],
        hub: MuxDevice,
        address: [u8; 6],
        owner: Arc<()>,
    ) {
        let uuid = self.device.uuid;
        self.hub_device = Some((hub, address));
        // Repeated snapshots must not restart active streams, but a different
        // dongle must replace the old BLE endpoint even if it is still draining.
        let same_owner = self
            .events_owner
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &owner));
        if self.sensors.is_running()
            && (same_owner || self.device.events_transport == EventsTransport::Wired)
        {
            info!(
                "AddEventsDevice for {uuid:02x?}: already streaming, ignoring redundant snapshot"
            );
            return;
        }

        info!("AddEventsDevice - adding BLE mux events device for {uuid:02x?}");
        self.connections.events_device = Some(events_device.clone());
        self.events_owner = Some(owner);
        self.protocol_version = events_protocol_version;
        self.stream_timers.paused = false;
        self.stream_timers.resume_armed = false;
        self.sensors = SensorStreams::start(&events_device, uuid, self.protocol_version).await;

        // A unit that started in BLE mode has no config yet; this is where it gets one.
        info!("Reading device config from BLE mux device for {uuid:02x?}");
        match tokio::time::timeout(SLOW_CALL_TIMEOUT, events_device.read_all_config()).await {
            Ok(Ok(settings)) => {
                info!("Updating device config for {uuid:02x?} from BLE mux");
                self.tracker
                    .set_cameras(CameraModels::from_settings(&settings));
            }
            Ok(Err(e)) => warn!("Failed to read device config from BLE mux {uuid:02x?}: {e}"),
            Err(_) => warn!("read_all_config timed out for BLE mux {uuid:02x?}"),
        }

        self.device.events_transport = EventsTransport::Bluetooth;
        self.device.capabilities.insert(DeviceCapabilities::EVENTS);
        self.device.events_connected = true;

        self.emit_capabilities_changed();
        self.send_device_update().await;

        // A unit with no cable has no USB control to read the net core
        // through; ask over the packet stream instead.
        if self.connections.control_device.is_none() && self.device.netcore_release == 0 {
            spawn_packet_netcore_probe(&self.self_tx, &events_device, self.protocol_version);
        }

        info!(
            "BLE mux events device added successfully for {uuid:02x?}, has_sensor_streams: {}",
            self.sensors.is_running()
        );
    }

    pub(super) async fn on_add_control_device(
        &mut self,
        control_device: VmDevice,
        dfu: bool,
        events_protocol_version: [u16; 3],
    ) {
        let uuid = self.device.uuid;
        info!("AddControlDevice - adding USB control device for {uuid:02x?}");

        self.refresh_firmware_version(&control_device).await;
        if let Some(mode) = read_control_transport_mode(&control_device, uuid).await {
            self.apply_transport_mode(mode);
            if mode == TransportMode::Usb {
                // A reboot or DFU may have replaced the old wired endpoint, or this
                // handler may have started over BLE; the USB link probed last is current.
                self.sensors.stop();
                self.protocol_version = events_protocol_version;
                self.connections.events_device = Some(control_device.clone());
                self.sensors =
                    SensorStreams::start(&control_device, uuid, self.protocol_version).await;
            }
        }

        self.connections.control_device = Some(control_device);
        self.device.capabilities.insert(DeviceCapabilities::CONTROL);
        if dfu {
            self.device.capabilities.insert(DeviceCapabilities::DFU);
        } else {
            self.device.capabilities.remove(DeviceCapabilities::DFU);
        }
        // With USB control attached, USB is the unit's primary transport.
        self.device.transport = Transport::Usb;

        self.send_device_update().await;
        self.emit_capabilities_changed();

        info!(
            "USB control device added successfully for {uuid:02x?}, capabilities: {:?}",
            self.device.capabilities
        );
    }

    /// Re-read the firmware version over USB control whenever it is (re)attached: after
    /// DFU the handler still holds the version from before the update.
    ///
    /// `protocol_version` is deliberately left alone: it gates events-protocol features (like
    /// the battery stream) and was set from the events protocol's ReadVersionResponse at spawn
    /// time. The control channel's protocol_semver is unrelated.
    async fn refresh_firmware_version(&mut self, control_device: &VmDevice) {
        let uuid = self.device.uuid;
        match tokio::time::timeout(CALL_TIMEOUT, control_device.read_version()).await {
            Ok(Ok(version)) => {
                let new_firmware = version.firmware_semver;
                if self.device.firmware_version == new_firmware {
                    info!(
                        "Read firmware version from USB control for {uuid:02x?}: {new_firmware:?}"
                    );
                } else {
                    info!(
                        "Updating firmware version for {uuid:02x?}: {:?} -> {new_firmware:?}",
                        self.device.firmware_version
                    );
                }
                self.device.firmware_version = new_firmware;
            }
            Ok(Err(e)) => {
                warn!("Failed to read firmware version from USB control for {uuid:02x?}: {e}");
            }
            Err(_) => warn!("read_version timed out for USB control {uuid:02x?}"),
        }
    }

    /// Make `events_transport` / `events_connected` reflect the firmware's current mode.
    fn apply_transport_mode(&mut self, mode: TransportMode) {
        let is_usb = mode == TransportMode::Usb;
        self.device.events_transport = if is_usb {
            EventsTransport::Wired
        } else {
            EventsTransport::Bluetooth
        };
        if is_usb {
            // USB transport mode: events come over USB, BLE is not needed.
            self.device.events_connected = true;
            self.device.capabilities.insert(DeviceCapabilities::EVENTS);
        } else if self.connections.events_device.is_none() {
            // BLE transport mode, no BLE events yet. (If BLE events are already streaming,
            // EVENTS and events_connected are correct as they are.)
            self.device.events_connected = false;
            self.device.capabilities.remove(DeviceCapabilities::EVENTS);
        }
    }

    pub(super) async fn on_net_core_release(&mut self, release: u16) {
        let was_known = self
            .device
            .capabilities
            .contains(DeviceCapabilities::NETCORE_KNOWN);
        if self.device.netcore_release == release && was_known {
            return;
        }
        info!(
            "Net core of {:02x?} reports release {release}",
            self.device.uuid
        );
        self.device.netcore_release = release;
        self.device
            .capabilities
            .insert(DeviceCapabilities::NETCORE_KNOWN);
        self.send_device_update().await;
    }

    /// Ends the handler unless BLE events can carry on without the cable.
    pub(super) async fn on_remove_control_device(&mut self) -> ControlFlow<()> {
        let uuid = self.device.uuid;
        info!("RemoveControlDevice - removing USB control for {uuid:02x?}");

        self.connections.control_device = None;
        self.device.capabilities.remove(DeviceCapabilities::CONTROL);
        // The DFU interface is only reachable over USB.
        self.device.capabilities.remove(DeviceCapabilities::DFU);

        // Events over an active BLE connection survive the cable going away. A purely
        // wired unit's events went with the control connection, so the handler ends.
        let has_independent_ble_events = self.connections.events_device.is_some()
            && self.device.events_transport == EventsTransport::Bluetooth
            && self.device.events_connected;
        if !has_independent_ble_events {
            // The wired events device is stale; ending makes the device leave the UI.
            self.connections.events_device = None;
            self.sensors.stop();
            info!("Device {uuid:02x?} has no independent events after USB disconnect, exiting");
            return ControlFlow::Break(());
        }

        self.device.transport = Transport::UsbMux;
        // The net core can only be read over USB control, and it may be
        // replaced while the cable is out, so the old reading is stale.
        self.device.netcore_release = 0;
        self.device
            .capabilities
            .remove(DeviceCapabilities::NETCORE_KNOWN);
        if let Some(events) = &self.connections.events_device {
            spawn_packet_netcore_probe(&self.self_tx, events, self.protocol_version);
        }
        // USB control is gone, so this is not a direct-USB handler any more; the next
        // USB re-plug must send AddControlDevice.
        self.context
            .direct_usb_handler_uuids
            .lock()
            .await
            .remove(&uuid);
        self.send_device_update().await;
        self.emit_capabilities_changed();
        info!("Device {uuid:02x?} continuing with BLE events only after USB disconnect");
        ControlFlow::Continue(())
    }

    /// Ends the handler unless USB control can carry on without the BLE events.
    pub(super) async fn on_remove_events_device(&mut self, owner: Arc<()>) -> ControlFlow<()> {
        // A unit may already have moved to another dongle while the old manager
        // finishes shutting down. Only that link's owner can detach its events.
        if self
            .events_owner
            .as_ref()
            .is_some_and(|current| !Arc::ptr_eq(current, &owner))
        {
            return ControlFlow::Continue(());
        }
        self.events_owner = None;
        self.hub_device = None;
        // A mux may have reported a unit that was already streaming over USB. Its
        // AddEventsDevice was ignored, so losing that mux must not drop wired events.
        if self.device.events_transport != EventsTransport::Bluetooth {
            return ControlFlow::Continue(());
        }
        let uuid = self.device.uuid;
        info!("RemoveEventsDevice - removing BLE mux events for {uuid:02x?}");

        self.connections.events_device = None;
        self.sensors.stop();
        self.device.capabilities.remove(DeviceCapabilities::EVENTS);
        self.device.events_connected = false;

        if self.connections.control_device.is_none() {
            info!("Device {uuid:02x?} has no control device, exiting");
            return ControlFlow::Break(());
        }

        info!("Device {uuid:02x?} has USB control, continuing without events");
        // Without BLE events the unit is not reached through the mux.
        if self.device.transport == Transport::UsbMux {
            self.device.transport = Transport::Usb;
        }
        self.send_device_update().await;
        self.emit_capabilities_changed();
        ControlFlow::Continue(())
    }

    /// Tell clients (event stream) that the unit's capabilities changed.
    fn emit_capabilities_changed(&self) {
        self.context
            .emit_device_event(&self.device, DeviceEventKind::CapabilitiesChanged);
    }

    /// Tell the server's device list about the unit's new metadata.
    async fn send_device_update(&mut self) {
        self.context
            .send_message(Message::UpdateDevice(self.device.clone()))
            .await;
    }
}

/// Re-read the transport mode from a freshly attached control device so
/// events_transport / events_connected match the firmware's current state.
async fn read_control_transport_mode(
    control_device: &VmDevice,
    uuid: [u8; 6],
) -> Option<TransportMode> {
    match tokio::time::timeout(CALL_TIMEOUT, control_device.get_transport_mode()).await {
        Ok(Ok(mode)) => {
            info!("AddControlDevice: transport mode for {uuid:02x?} = {mode:?}");
            Some(mode)
        }
        Ok(Err(e)) => {
            warn!("AddControlDevice: failed to read transport mode for {uuid:02x?}: {e}");
            None
        }
        Err(_) => {
            warn!("AddControlDevice: get_transport_mode timed out for {uuid:02x?}");
            None
        }
    }
}
