use std::sync::Arc;

use odyssey_hub_common::device::Device;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::device_tasks::DeviceTaskMessage;

type Entry = (Device, mpsc::Sender<DeviceTaskMessage>);

/// The connected devices, each with the channel that reaches its device task.
///
/// Cloning shares the list. Devices are identified by UUID alone, the transport
/// is only a property of the entry. No method holds the lock across an await.
#[derive(Clone, Default)]
pub(crate) struct DeviceList(Arc<Mutex<Vec<Entry>>>);

impl DeviceList {
    pub(crate) fn snapshot(&self) -> Vec<Device> {
        self.0
            .lock()
            .iter()
            .map(|(device, _)| device.clone())
            .collect()
    }

    pub(crate) fn find(&self, uuid: [u8; 6]) -> Option<Device> {
        let entries = self.0.lock();
        let (device, _) = entries.iter().find(|(device, _)| device.uuid == uuid)?;
        Some(device.clone())
    }

    /// The channel to a device's task, if the device is listed.
    pub(crate) fn task_sender(&self, uuid: [u8; 6]) -> Option<mpsc::Sender<DeviceTaskMessage>> {
        let entries = self.0.lock();
        let (_, sender) = entries.iter().find(|(device, _)| device.uuid == uuid)?;
        Some(sender.clone())
    }

    /// Adds a device, or replaces the listed one with the same UUID. A reconnect
    /// without a command channel keeps the channel the entry already has.
    pub(crate) fn upsert(&self, device: Device, commands: Option<mpsc::Sender<DeviceTaskMessage>>) {
        let mut entries = self.0.lock();
        let existing = entries
            .iter()
            .position(|(listed, _)| listed.uuid == device.uuid);
        match (existing, commands) {
            (Some(index), Some(commands)) => {
                tracing::info!("Device {:02x?} already exists, updating", device.uuid);
                entries[index] = (device, commands);
            }
            (Some(index), None) => {
                tracing::info!("Device {:02x?} already exists, updating", device.uuid);
                entries[index].0 = device;
            }
            (None, Some(commands)) => entries.push((device, commands)),
            (None, None) => {
                // A control-only device still gets listed. Nothing reads from
                // this channel, so a command sent to it fails as "unavailable".
                let (unconnected, _) = mpsc::channel(1);
                entries.push((device, unconnected));
            }
        }
    }

    pub(crate) fn remove(&self, uuid: [u8; 6]) {
        let mut entries = self.0.lock();
        let Some(index) = entries.iter().position(|(device, _)| device.uuid == uuid) else {
            tracing::warn!("Device {uuid:02x?} not found in device list for disconnect");
            return;
        };
        tracing::info!("Removing device {uuid:02x?} from device list (index {index})");
        entries.remove(index);
    }

    /// Replaces the listed device that has the same UUID, keeping its channel.
    pub(crate) fn update(&self, updated: Device) {
        let mut entries = self.0.lock();
        let Some((listed, _)) = entries
            .iter_mut()
            .find(|(device, _)| device.uuid == updated.uuid)
        else {
            tracing::warn!(
                "Device {:02x?} not found in device list for update",
                updated.uuid
            );
            return;
        };
        tracing::info!(
            "Updating device {:02x?} capabilities from {:?} to {:?}",
            updated.uuid,
            listed.capabilities,
            updated.capabilities
        );
        *listed = updated;
    }

    pub(crate) fn rename(&self, uuid: [u8; 6], name: &str) {
        let mut entries = self.0.lock();
        if let Some((device, _)) = entries.iter_mut().find(|(device, _)| device.uuid == uuid) {
            device.name = Device::name_bytes(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use odyssey_hub_common::device::{DeviceCapabilities, EventsTransport, Transport};
    use odyssey_hub_common::firmware_version::FirmwareVersion;

    use super::*;

    fn device(uuid_tail: u8) -> Device {
        Device {
            uuid: [0, 0, 0, 0, 0, uuid_tail],
            transport: Transport::Usb,
            capabilities: DeviceCapabilities::empty(),
            firmware_identity: FirmwareVersion::default(),
            firmware_version: [0; 3],
            events_transport: EventsTransport::Wired,
            events_connected: false,
            product_id: 0,
            name: Device::name_bytes(""),
            netcore_release: 0,
        }
    }

    #[test]
    fn upsert_without_a_channel_keeps_the_existing_one() {
        let list = DeviceList::default();
        let (commands, mut received) = mpsc::channel(1);
        list.upsert(device(1), Some(commands));

        let mut reconnected = device(1);
        reconnected.events_connected = true;
        list.upsert(reconnected, None);

        assert_eq!(list.snapshot().len(), 1);
        assert!(
            list.find([0, 0, 0, 0, 0, 1])
                .is_some_and(|d| d.events_connected)
        );
        let sender = list
            .task_sender([0, 0, 0, 0, 0, 1])
            .expect("device is listed");
        sender
            .try_send(DeviceTaskMessage::ClearZero)
            .expect("original channel is kept");
        received.try_recv().expect("the clear was forwarded");
    }

    #[test]
    fn control_only_device_is_listed_with_an_unusable_channel() {
        let list = DeviceList::default();
        list.upsert(device(2), None);

        let sender = list
            .task_sender([0, 0, 0, 0, 0, 2])
            .expect("device is listed");
        assert!(sender.try_send(DeviceTaskMessage::ClearZero).is_err());
    }

    #[test]
    fn remove_and_update_only_touch_the_matching_uuid() {
        let list = DeviceList::default();
        list.upsert(device(1), None);
        list.upsert(device(2), None);

        let mut changed = device(2);
        changed.firmware_version = [1, 2, 3];
        list.update(changed);
        list.update(device(9));
        list.remove([0, 0, 0, 0, 0, 1]);
        list.remove([0, 0, 0, 0, 0, 9]);

        let remaining = list.snapshot();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].uuid, [0, 0, 0, 0, 0, 2]);
        assert_eq!(remaining[0].firmware_version, [1, 2, 3]);
    }

    #[test]
    fn rename_truncates_to_the_name_buffer() {
        let list = DeviceList::default();
        list.upsert(device(1), None);

        list.rename([0, 0, 0, 0, 0, 1], &"n".repeat(40));

        let renamed = list.find([0, 0, 0, 0, 0, 1]).expect("device is listed");
        assert_eq!(renamed.name(), "n".repeat(32));
    }
}
