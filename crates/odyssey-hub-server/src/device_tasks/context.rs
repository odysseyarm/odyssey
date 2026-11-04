use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use nalgebra::Isometry3;
use odyssey_common::ScreenCalibration;
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, watch};

use super::messages::{DeviceTaskMessage, Message};

/// BLE address (or, for wired units, the device UUID that stands in for it).
pub(super) type DeviceAddress = [u8; 6];

/// One slot per screen id, `0..=MAX_SCREEN_ID`.
const SCREEN_SLOTS: usize = (odyssey_common::MAX_SCREEN_ID + 1) as usize;

pub(super) type ScreenCalibrationList = ArrayVec<(u8, ScreenCalibration<f32>), SCREEN_SLOTS>;

/// Screen calibrations, swapped atomically by the server when the user recalibrates.
pub(super) type ScreenCalibrations = Arc<ArcSwap<ScreenCalibrationList>>;

/// Saved zero offsets keyed by device UUID.
pub(super) type DeviceOffsets = Arc<Mutex<HashMap<DeviceAddress, Isometry3<f32>>>>;

/// Configured shot delay (milliseconds) per device UUID.
pub(super) type ShotDelays = Arc<RwLock<HashMap<DeviceAddress, u16>>>;

/// Live shot-delay subscriptions per device UUID, so a changed delay reaches a running handler.
pub(super) type ShotDelayWatchers = Arc<RwLock<HashMap<DeviceAddress, watch::Sender<u32>>>>;

/// Device name by BLE address, shared between hub managers and direct USB handlers.
/// Updated on USB rename so BLE reconnects use the current name.
pub(super) type BondNames = Arc<Mutex<HashMap<DeviceAddress, String>>>;

/// UUIDs whose handlers already have a direct-USB control connection (including
/// handlers that started over BLE). A scan must not repeatedly replace that endpoint.
/// Removed when the cable or handler goes away.
pub(super) type DirectUsbHandlerUuids = Arc<Mutex<HashSet<DeviceAddress>>>;

/// State and outboxes every device-facing task needs. Cloning is cheap (all shared).
#[derive(Clone)]
pub(super) struct DeviceTaskContext {
    pub(super) message_channel: mpsc::Sender<Message>,
    pub(super) screen_calibrations: ScreenCalibrations,
    pub(super) device_offsets: DeviceOffsets,
    pub(super) device_shot_delays: ShotDelays,
    pub(super) shot_delay_watch: ShotDelayWatchers,
    pub(super) event_sender: broadcast::Sender<Event>,
    pub(super) direct_usb_handler_uuids: DirectUsbHandlerUuids,
    pub(super) bond_names: BondNames,
}

/// A running per-device handler task and the channel that reaches it.
#[derive(Debug)]
pub(super) struct RunningDevice {
    pub(super) commands: mpsc::Sender<DeviceTaskMessage>,
    pub(super) task: tokio::task::JoinHandle<()>,
}

impl Drop for RunningDevice {
    fn drop(&mut self) {
        // A dropped manager or a replaced slot must not detach a handler that
        // still owns USB streams and event broadcasters.
        self.task.abort();
    }
}

/// Handler tasks by device UUID; shared between the USB scan loop and every hub manager.
pub(super) type DeviceHandles = Arc<Mutex<HashMap<DeviceAddress, RunningDevice>>>;

impl DeviceTaskContext {
    /// Broadcast a device event. Having no subscribers is normal, so a failed send is ignored.
    pub(super) fn emit_device_event(&self, device: &Device, kind: DeviceEventKind) {
        let _ = self
            .event_sender
            .send(Event::DeviceEvent(DeviceEvent(device.clone(), kind)));
    }

    /// Tell the server's device list about a change. The list outlives every task, so a
    /// failed send only happens during shutdown and is ignored.
    pub(super) async fn send_message(&self, message: Message) {
        let _ = self.message_channel.send(message).await;
    }
}
