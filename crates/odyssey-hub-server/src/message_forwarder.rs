use odyssey_hub_common::device::Device;
use tokio::sync::{broadcast, mpsc};

use crate::device_list::DeviceList;
use crate::device_tasks::{self, DeviceTaskMessage, DongleInfo, DongleTaskMessage};
use crate::dongle_list::DongleList;

/// Applies what the device tasks report to the shared device and dongle lists,
/// then tells list subscribers that something changed.
///
/// Every method runs to completion without awaiting, so the list locks are
/// never held across a suspension point.
pub(crate) struct MessageForwarder {
    pub device_list: DeviceList,
    pub dongle_list: DongleList,
    pub device_list_changed: broadcast::Sender<()>,
    pub dongle_list_changed: broadcast::Sender<()>,
}

impl MessageForwarder {
    pub(crate) fn forward(&self, message: device_tasks::Message) {
        match message {
            device_tasks::Message::Connect(device, channels) => {
                self.device_connected(device, channels.commands);
            }
            device_tasks::Message::Disconnect(device) => self.device_disconnected(&device),
            device_tasks::Message::UpdateDevice(device) => self.device_updated(device),
            device_tasks::Message::UpdateName(uuid, name) => self.device_renamed(uuid, &name),
            device_tasks::Message::DongleConnect(info, commands) => {
                self.dongle_connected(info, commands);
            }
            device_tasks::Message::DongleUpdate(info) => self.dongle_updated(info),
            device_tasks::Message::DongleDisconnect(id) => self.dongle_disconnected(&id),
        }
    }

    fn device_connected(&self, device: Device, commands: Option<mpsc::Sender<DeviceTaskMessage>>) {
        let uuid = device.uuid;
        tracing::info!("Processing connect for device {uuid:02x?}");
        self.device_list.upsert(device, commands);
        tracing::info!(
            "Added/updated device {uuid:02x?} in device list, sending list_change notification"
        );
        let _ = self.device_list_changed.send(());
    }

    fn device_disconnected(&self, device: &Device) {
        tracing::info!("Processing disconnect for device {:02x?}", device.uuid);
        self.device_list.remove(device.uuid);
        tracing::info!(
            "Sending list_change notification for disconnect of {:02x?}",
            device.uuid
        );
        let _ = self.device_list_changed.send(());
    }

    fn device_updated(&self, device: Device) {
        let uuid = device.uuid;
        tracing::info!("Processing device update for {uuid:02x?}");
        self.device_list.update(device);
        tracing::info!("Sending list_change notification for update of {uuid:02x?}");
        let _ = self.device_list_changed.send(());
    }

    fn device_renamed(&self, uuid: [u8; 6], name: &str) {
        self.device_list.rename(uuid, name);
        let _ = self.device_list_changed.send(());
    }

    fn dongle_connected(&self, info: DongleInfo, commands: mpsc::Sender<DongleTaskMessage>) {
        tracing::info!("Processing dongle connect: {}", info.id);
        self.dongle_list.upsert(info, commands);
        let _ = self.dongle_list_changed.send(());
    }

    fn dongle_updated(&self, info: DongleInfo) {
        tracing::debug!(
            "Processing dongle update: {} ({} connected devices)",
            info.id,
            info.connected_devices.len()
        );
        self.dongle_list.update(info);
        let _ = self.dongle_list_changed.send(());
    }

    fn dongle_disconnected(&self, id: &str) {
        tracing::info!("Processing dongle disconnect: {id}");
        self.dongle_list.remove(id);
        let _ = self.dongle_list_changed.send(());
    }
}
