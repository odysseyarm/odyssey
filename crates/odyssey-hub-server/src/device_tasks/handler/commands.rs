//! Control messages that make the unit do something: vendor packets, register and config
//! access, flashing, pairing, bonds, naming and transport mode.

use std::ops::ControlFlow;
use std::time::Duration;

use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::DeviceEventKind;
use odyssey_protocol::control::device::TransportMode;
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::{ConfigKind, GeneralConfig, PacketData};
use tracing::{debug, info, warn};

use super::{CALL_TIMEOUT, DeviceHandler, SLOW_CALL_TIMEOUT, call_with_timeout};
use crate::device_tasks::messages::{Message, Reply};

/// Longest vendor payload the firmware accepts; the rest is dropped.
const MAX_VENDOR_PAYLOAD_LEN: usize = 98;

/// Reboot is fire-and-forget: the unit disappears before it could acknowledge.
const REBOOT_REQUEST_TIMEOUT: Duration = Duration::from_millis(500);

/// RPCs address sensors by index: 0 is the near-field camera, anything else the wide-field one.
fn sensor_port(port: u8) -> odyssey_protocol::Port {
    if port == 0 {
        odyssey_protocol::Port::Nf
    } else {
        odyssey_protocol::Port::Wf
    }
}

/// RPC config kinds: 0 is the impact threshold, 1 the suppression window.
fn config_kind_from_rpc(kind: u8) -> Option<ConfigKind> {
    match kind {
        0 => Some(ConfigKind::ImpactThreshold),
        1 => Some(ConfigKind::SuppressMs),
        _ => None,
    }
}

fn config_value(config: GeneralConfig) -> u8 {
    match config {
        GeneralConfig::ImpactThreshold(value) | GeneralConfig::SuppressMs(value) => value,
        _ => 0,
    }
}

impl DeviceHandler {
    pub(super) async fn on_write_vendor(&mut self, tag: u8, data: &[u8]) {
        debug!(
            "WriteVendor tag={} len={} for {:02x?}",
            tag,
            data.len(),
            self.device.uuid
        );
        let clipped_len = data.len().min(MAX_VENDOR_PAYLOAD_LEN);
        let Some(events_device) = self.connections.events_device.as_ref() else {
            warn!("Cannot write vendor packet: no events device available");
            return;
        };
        if let Err(e) = events_device.write_vendor(tag, &data[..clipped_len]).await {
            warn!("Failed to send vendor packet: {}", e);
        }
    }

    /// The device register access goes through. Register access is a packet-protocol
    /// operation, so it needs a connected events device.
    fn register_access_device(&self) -> Result<VmDevice, &'static str> {
        if !self.device.events_connected {
            return Err("events transport not connected");
        }
        self.connections
            .events_device
            .clone()
            .ok_or("events device not available")
    }

    pub(super) async fn on_read_register(
        &mut self,
        port: u8,
        bank: u8,
        address: u8,
        reply: Reply<u8>,
    ) {
        let device = match self.register_access_device() {
            Ok(device) => device,
            Err(reason) => {
                let _ = reply.send(Err(reason.into()));
                return;
            }
        };
        self.pause_sensor_streams(&device, "ReadRegister").await;
        let result = call_with_timeout(
            CALL_TIMEOUT,
            "read_register",
            device.read_register(sensor_port(port), bank, address),
        )
        .await;
        let _ = reply.send(result);
        self.stream_timers.arm_resume();
    }

    pub(super) async fn on_write_register(
        &mut self,
        port: u8,
        bank: u8,
        address: u8,
        data: u8,
        reply: Reply<()>,
    ) {
        let device = match self.register_access_device() {
            Ok(device) => device,
            Err(reason) => {
                let _ = reply.send(Err(reason.into()));
                return;
            }
        };
        self.pause_sensor_streams(&device, "WriteRegister").await;
        let result = call_with_timeout(
            CALL_TIMEOUT,
            "write_register",
            device.write_register(sensor_port(port), bank, address, data),
        )
        .await;
        let _ = reply.send(result);
        self.stream_timers.arm_resume();
    }

    pub(super) async fn on_read_config(&mut self, kind: u8, reply: Reply<u8>) {
        let Some(config_kind) = config_kind_from_rpc(kind) else {
            let _ = reply.send(Err(format!("unknown config kind: {kind}")));
            return;
        };
        // read_config uses packet protocol — works over BLE and USB alike.
        // Only read_all_config (bulk camera data) is gated on USB transport mode.
        if let Some(device) = self.connections.events_device.clone() {
            self.pause_sensor_streams(&device, "ReadConfig").await;
            let result =
                call_with_timeout(CALL_TIMEOUT, "read_config", device.read_config(config_kind))
                    .await
                    .map(config_value);
            let _ = reply.send(result);
            self.stream_timers.arm_resume();
        } else if let Some(device) = self.connections.control_device.as_ref() {
            let result = call_with_timeout(
                CALL_TIMEOUT,
                "read_config_ctrl",
                device.read_config_ctrl(config_kind),
            )
            .await
            .map(config_value);
            let _ = reply.send(result);
        } else {
            let _ = reply.send(Err("no device available for read_config".into()));
        }
    }

    pub(super) async fn on_write_config(&mut self, kind: u8, value: u8, reply: Reply<()>) {
        let config = match kind {
            0 => GeneralConfig::ImpactThreshold(value),
            1 => GeneralConfig::SuppressMs(value),
            _ => {
                let _ = reply.send(Err(format!("unknown config kind: {kind}")));
                return;
            }
        };
        // write_config uses packet protocol — works over BLE and USB alike.
        if let Some(device) = self.connections.events_device.clone() {
            self.pause_sensor_streams(&device, "WriteConfig").await;
            let result =
                call_with_timeout(CALL_TIMEOUT, "write_config", device.write_config(config)).await;
            let _ = reply.send(result);
            self.stream_timers.arm_resume();
        } else if let Some(device) = self.connections.control_device.as_ref() {
            let result = call_with_timeout(
                CALL_TIMEOUT,
                "write_config_ctrl",
                device.write_config_ctrl(config),
            )
            .await;
            let _ = reply.send(result);
        } else {
            let _ = reply.send(Err("no device available for write_config".into()));
        }
    }

    pub(super) async fn on_flash_settings(&mut self, reply: Reply<()>) {
        // flash_settings uses packet protocol — works over BLE and USB alike.
        let result = if let Some(device) = self.connections.events_device.as_ref() {
            call_with_timeout(CALL_TIMEOUT, "flash_settings", device.flash_settings()).await
        } else if let Some(device) = self.connections.control_device.as_ref() {
            call_with_timeout(
                SLOW_CALL_TIMEOUT,
                "flash_settings_ctrl",
                device.flash_settings_ctrl(),
            )
            .await
        } else {
            Err("no device available for flash_settings".into())
        };
        let _ = reply.send(result);
    }

    pub(super) async fn on_start_pairing(&mut self, timeout_ms: u32, reply: Reply<()>) {
        let uuid = self.device.uuid;
        let Some(device) = self.connections.control_device.as_ref() else {
            let _ = reply.send(Err("no control device available".into()));
            return;
        };
        info!("StartPairing: sending start_pairing({timeout_ms}ms) to device {uuid:02x?}");
        let result = call_with_timeout(
            SLOW_CALL_TIMEOUT,
            "start_pairing",
            device.start_pairing(timeout_ms),
        )
        .await;
        info!("StartPairing result for {uuid:02x?}: {result:?}");
        if result.is_ok() {
            self.spawn_pairing_result_watcher(device.clone());
        }
        let _ = reply.send(result);
    }

    /// Report the outcome of a pairing attempt as a `PairingResult` event once the unit
    /// settles it. Detached on purpose: it only waits for one answer from the unit and
    /// broadcasts it, and it ends on its own when the link drops.
    fn spawn_pairing_result_watcher(&self, device: VmDevice) {
        let context = self.context.clone();
        let reporting_device = self.device.clone();
        tokio::spawn(async move {
            let (success, paired_address, paired_name, error) =
                match device.wait_pairing_event().await {
                    Ok(bonded) => (true, bonded.uuid, bonded.name.to_string(), String::new()),
                    Err(e) => (false, [0; 6], String::new(), e.to_string()),
                };
            context.emit_device_event(
                &reporting_device,
                DeviceEventKind::PairingResult {
                    success,
                    paired_address,
                    paired_name,
                    error,
                },
            );
        });
    }

    pub(super) async fn on_cancel_pairing(&mut self, reply: Reply<()>) {
        let uuid = self.device.uuid;
        let Some(device) = self.connections.control_device.as_ref() else {
            warn!("CancelPairing: no control device for {uuid:02x?}");
            let _ = reply.send(Err("no control device available".into()));
            return;
        };
        info!("CancelPairing: sending cancel to device {uuid:02x?}");
        let result =
            call_with_timeout(CALL_TIMEOUT, "cancel_pairing", device.cancel_pairing()).await;
        info!("CancelPairing result for {uuid:02x?}: {result:?}");
        let _ = reply.send(result);
    }

    pub(super) async fn on_clear_bond(&mut self, reply: Reply<()>) {
        let uuid = self.device.uuid;
        let Some(device) = self.connections.control_device.as_ref() else {
            warn!("ClearBond: no control device for {uuid:02x?}");
            let _ = reply.send(Err("no control device available".into()));
            return;
        };
        info!("ClearBond: sending clear to device {uuid:02x?}");
        let result = call_with_timeout(CALL_TIMEOUT, "clear_bond", device.clear_bond()).await;
        info!("ClearBond result for {uuid:02x?}: {result:?}");
        let _ = reply.send(result);
    }

    /// Ends the handler whether or not the switch worked: the unit is rebooted, and
    /// reconnects through the USB scan.
    pub(super) async fn on_set_transport_mode(
        &mut self,
        usb_mode: bool,
        reply: Reply<bool>,
    ) -> ControlFlow<()> {
        let uuid = self.device.uuid;
        let Some(device) = self.connections.control_device.clone() else {
            warn!("SetTransportMode: no control device for {uuid:02x?}");
            let _ = reply.send(Err("no control device available".into()));
            return ControlFlow::Continue(());
        };
        info!("SetTransportMode: usb_mode={usb_mode} for device {uuid:02x?}");
        let result = call_with_timeout(
            CALL_TIMEOUT,
            "set_transport_mode",
            device.set_transport_mode(usb_mode),
        )
        .await
        .map(|mode| matches!(mode, TransportMode::Usb));
        info!("SetTransportMode result for {uuid:02x?}: {result:?}");
        // The firmware's SetTransportMode handler persists the mode itself
        // via persist_transport_mode, so no separate FlashSettings is needed.
        // Sending FlashSettings would corrupt legacy board settings because
        // atslite-common's GeneralSettings uses a different CBOR field layout
        // than common's GeneralSettings (transport_mode at field #0 vs #7).
        // Reply to the RPC caller before rebooting.
        let _ = reply.send(result);
        // Reboot the device so it comes back clean in the new mode.
        // We don't wait for the full ack timeout — just fire and break
        // immediately so the device disappears from the UI before the
        // reboot completes. This prevents stale RPC calls (e.g.
        // ReadRegister) from hitting a dead device while it is resetting.
        info!("Rebooting device {uuid:02x?} to apply transport mode change");
        let _ = tokio::time::timeout(REBOOT_REQUEST_TIMEOUT, device.reboot()).await;
        ControlFlow::Break(())
    }

    pub(super) async fn on_set_device_name(&mut self, name: String, reply: Reply<()>) {
        let uuid = self.device.uuid;
        info!("SetDeviceName: name={name:?} for device {uuid:02x?}");
        let result = self.write_device_name(&name).await;
        info!("SetDeviceName result for {uuid:02x?}: {result:?}");
        if result.is_ok() {
            self.device.name = Device::name_bytes(&name);
            self.context
                .bond_names
                .lock()
                .await
                .insert(uuid, name.clone());
            if let Some(hub) = &self.hub_device {
                let bond_name = odyssey_protocol::DeviceName::from_str_lossy(&name);
                if let Err(e) = hub.update_bond_name(uuid, bond_name.into()).await {
                    warn!("update_bond_name failed for {uuid:02x?}: {e}");
                }
            }
            self.context
                .send_message(Message::UpdateName(uuid, name))
                .await;
        }
        let _ = reply.send(result);
    }

    async fn write_device_name(&mut self, name: &str) -> Result<(), String> {
        if let Some(device) = self.connections.control_device.as_ref() {
            // USB EP0 path
            return call_with_timeout(SLOW_CALL_TIMEOUT, "set_device_name", device.set_name(name))
                .await;
        }
        let Some(device) = self.connections.events_device.as_ref() else {
            warn!("SetDeviceName: no device for {:02x?}", self.device.uuid);
            return Err("no device available".into());
        };
        // BLE mux path
        let packet_name = odyssey_protocol::DeviceName::from_str_lossy(name);
        let response = call_with_timeout(
            SLOW_CALL_TIMEOUT,
            "set_device_name",
            device.request(PacketData::SetDeviceName(packet_name)),
        )
        .await?;
        match response {
            PacketData::SetDeviceNameResponse(odyssey_protocol::SetDeviceNameResult::Ok()) => {
                Ok(())
            }
            _ => Err("unexpected response to SetDeviceName".into()),
        }
    }
}
