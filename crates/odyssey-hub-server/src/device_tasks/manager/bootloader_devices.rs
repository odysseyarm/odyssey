//! Units that cannot talk to the hub but are sitting in their DFU bootloader. They are
//! put into the device list anyway, because that is the only way to flash them.

use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};
use tracing::{info, warn};

use super::{PresentUsbDevices, UsbDeviceManager};
use crate::device_tasks::context::DeviceAddress;
use crate::device_tasks::messages::{DeviceChannels, Message};
use crate::firmware::{DFU_CLASS, DFU_PROTOCOL_DFU_MODE, DFU_SUBCLASS};

/// A unit in its bootloader, as the device list knows it.
pub(super) struct BootloaderDevice {
    uuid: DeviceAddress,
    /// The rest of the system knows a device by the product id its application enumerates
    /// under, which is not the one the bootloader uses.
    app_pid: u16,
}

impl BootloaderDevice {
    /// No application is running, so there is no firmware version, name or events to report.
    fn device_list_entry(&self) -> Device {
        Device {
            uuid: self.uuid,
            transport: Transport::Usb,
            capabilities: DeviceCapabilities::new(DeviceCapabilities::DFU),
            firmware_version: [0, 0, 0],
            events_transport: EventsTransport::Wired,
            events_connected: false,
            product_id: self.app_pid,
            name: Device::name_bytes(""),
            netcore_release: 0,
        }
    }
}

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

        let pid = dev_info.product_id();
        let Some(uuid) = self.bootloader_uuid(dev_info) else {
            warn!(
                "Device PID 0x{:04x} is in DFU mode but reports no usable serial and was not seen before — cannot surface for update",
                pid
            );
            return;
        };
        present.bootloader_ids.insert(direct_usb_id.to_owned());

        let already_tracked = self.device_handles.lock().await.contains_key(&uuid)
            || self
                .bootloader_devices
                .values()
                .any(|known| known.uuid == uuid);
        if already_tracked {
            return;
        }

        let bootloader_release = crate::firmware::bootloader_release(dev_info.device_version());
        info!(
            "Device {:02x?} is in DFU mode (PID 0x{:04x}, bootloader {:?}), surfacing for firmware update (connect error: {})",
            uuid, pid, bootloader_release, connect_error
        );
        let bootloader_device = BootloaderDevice {
            uuid,
            app_pid: crate::firmware::app_pid(pid),
        };
        let entry = bootloader_device.device_list_entry();
        self.bootloader_devices
            .insert(direct_usb_id.to_owned(), bootloader_device);
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
        // device last seen under this product id; a device that has been in the bootloader
        // since before the app started has never been seen, so it gets a stand-in instead,
        // which is the only way it reaches the device list and therefore the only way it can
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

    /// Remove bootloader entries whose USB device is no longer present.
    pub(super) async fn reconcile_bootloader_disconnects(&mut self, present: &PresentUsbDevices) {
        let missing_ids: Vec<String> = self
            .bootloader_devices
            .keys()
            .filter(|id| !present.bootloader_ids.contains(*id))
            .cloned()
            .collect();
        for id in missing_ids {
            let Some(bootloader_device) = self.bootloader_devices.remove(&id) else {
                continue;
            };
            tracing::info!(
                "DFU device missing from scan, disconnecting: id={} uuid={:02x?}",
                id,
                bootloader_device.uuid
            );
            self.context
                .send_message(Message::Disconnect(bootloader_device.device_list_entry()))
                .await;
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
