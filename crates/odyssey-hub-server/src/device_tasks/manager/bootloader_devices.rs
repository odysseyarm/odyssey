//! USB DFU-only peripherals and bootloader recovery entries without hub protocol handlers.

use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};
use odyssey_hub_common::firmware_version::FirmwareVersion;
use tracing::{info, warn};

use super::{PresentUsbDevices, UsbDeviceManager};
use crate::device_tasks::context::DeviceAddress;
use crate::device_tasks::messages::{DeviceChannels, Message};
use crate::firmware::{DFU_CLASS, DFU_PROTOCOL_DFU_MODE, DFU_SUBCLASS};

const DFU_PROTOCOL_RUNTIME: u8 = 0x01;

impl UsbDeviceManager {
    /// Called when connecting to a unit failed: surface it for firmware update if it really
    /// is sitting in the bootloader.
    pub(super) async fn on_unconnectable_usb_device(
        &mut self,
        dev_info: &nusb::DeviceInfo,
        direct_usb_id: &str,
        connect_error: &odyssey_usb::Error,
        present: &mut PresentUsbDevices,
    ) {
        tracing::debug!("Failed to connect to USB device: {}", connect_error);
        // A running handler has this USB device's interface claimed, so the connect
        // failure is expected and the unit is not in its bootloader.
        if let Some(existing_uuid) = self.direct_usb_id_to_uuid.get(direct_usb_id)
            && self.device_handles.lock().await.contains_key(existing_uuid)
        {
            return;
        }
        if !is_in_bootloader(dev_info) {
            return;
        }
        self.track_dfu_device(dev_info, direct_usb_id, present, None, None)
            .await;
    }

    pub(super) async fn on_marker_control_board(
        &mut self,
        dev_info: &nusb::DeviceInfo,
        present: &mut PresentUsbDevices,
    ) {
        let (application_version, firmware_identity) = if is_in_bootloader(dev_info) {
            (None, super::read_firmware_identity(dev_info))
        } else {
            if !dev_info.interfaces().any(|interface| {
                interface.class() == DFU_CLASS
                    && interface.subclass() == DFU_SUBCLASS
                    && interface.protocol() == DFU_PROTOCOL_RUNTIME
            }) {
                return;
            }
            let firmware_identity = super::read_firmware_identity(dev_info);
            let application_version = firmware_identity
                .and_then(|version| version.known().map(|version| version.core))
                .unwrap_or([0; 3]);
            (Some(application_version), firmware_identity)
        };
        let direct_usb_id = format!("{:?}", dev_info.id());
        self.track_dfu_device(
            dev_info,
            &direct_usb_id,
            present,
            application_version,
            firmware_identity,
        )
        .await;
    }

    /// `None` means bootloader. Runtime-only peripherals report their application version.
    async fn track_dfu_device(
        &mut self,
        dev_info: &nusb::DeviceInfo,
        direct_usb_id: &str,
        present: &mut PresentUsbDevices,
        application_version: Option<[u32; 3]>,
        firmware_identity: Option<FirmwareVersion>,
    ) {
        let pid = dev_info.product_id();
        let Some(uuid) = self.bootloader_uuid(dev_info) else {
            warn!(
                "DFU device PID 0x{:04x} reports no usable serial and was not seen before — cannot surface for update",
                pid
            );
            return;
        };
        present.dfu_ids.insert(direct_usb_id.to_owned());

        let already_tracked = self.device_handles.lock().await.contains_key(&uuid)
            || self.dfu_devices.values().any(|known| known.uuid == uuid);
        if already_tracked {
            return;
        }

        info!("Surfacing USB DFU device {:02x?} (PID 0x{:04x})", uuid, pid);
        let mut capabilities = DeviceCapabilities::new(DeviceCapabilities::DFU);
        if application_version.is_some() {
            capabilities.insert(DeviceCapabilities::DFU_RUNTIME_ONLY);
        }
        let entry = Device {
            uuid,
            transport: Transport::Usb,
            capabilities,
            firmware_identity: firmware_identity.unwrap_or_default(),
            firmware_version: application_version.unwrap_or([0, 0, 0]),
            events_transport: EventsTransport::Wired,
            events_connected: false,
            product_id: if application_version.is_some() {
                pid
            } else {
                crate::firmware::app_pid(pid)
            },
            name: Device::name_bytes(dev_info.product_string().unwrap_or("")),
            netcore_release: 0,
        };
        self.dfu_devices
            .insert(direct_usb_id.to_owned(), entry.clone());
        self.context
            .send_message(Message::Connect(entry, DeviceChannels { commands: None }))
            .await;
    }

    /// The UUID a bootloader goes by. The serial number encodes it in both modes.
    fn bootloader_uuid(&self, dev_info: &nusb::DeviceInfo) -> Option<DeviceAddress> {
        use crate::device_tasks::usb_probe::uuid_from_usb_serial;

        if let Some(uuid) = uuid_from_usb_serial(dev_info.serial_number()) {
            return Some(uuid);
        }
        // Bootloaders that predate per-device serials cannot identify themselves. Prefer the
        // device last seen under this product id. A device that has been in the bootloader
        // since before the app started has never been seen, so it gets a stand-in instead.
        // That is the only way it reaches the device list and therefore the only way it can
        // be recovered.
        if !crate::migration::is_placeholder_serial(dev_info.serial_number()) {
            return None;
        }
        let pid = dev_info.product_id();
        self.last_uuid_by_pid
            .get(&pid)
            .copied()
            .or_else(|| crate::migration::recovery_uuid(dev_info.vendor_id(), pid))
    }

    /// Remove DFU-only entries whose USB device is no longer present.
    pub(super) async fn reconcile_dfu_disconnects(&mut self, present: &PresentUsbDevices) {
        let missing_ids: Vec<String> = self
            .dfu_devices
            .keys()
            .filter(|id| !present.dfu_ids.contains(*id))
            .cloned()
            .collect();
        for id in missing_ids {
            let Some(device) = self.dfu_devices.remove(&id) else {
                continue;
            };
            tracing::info!(
                "DFU device missing from scan, disconnecting: id={} uuid={:02x?}",
                id,
                device.uuid
            );
            self.context.send_message(Message::Disconnect(device)).await;
        }
    }
}

/// An application-mode ATS Lite also exposes a class 0xFE interface, for the DFU *runtime*
/// descriptor it uses to accept a detach, so the class alone says nothing: only protocol
/// 0x02 means the bootloader is running.
fn is_in_bootloader(dev_info: &nusb::DeviceInfo) -> bool {
    dev_info.interfaces().any(|interface| {
        interface.class() == DFU_CLASS
            && interface.subclass() == DFU_SUBCLASS
            && interface.protocol() == DFU_PROTOCOL_DFU_MODE
    })
}
