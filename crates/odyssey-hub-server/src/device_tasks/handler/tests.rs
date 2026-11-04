//! The handler's message loop, driven without hardware: a handler with no connections
//! still serves every message that does not need a device.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use nalgebra::{Isometry3, Point2, Translation3};
use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, oneshot};

use super::{DeviceConnections, DeviceHandler, DeviceHandlerInit};
use crate::device_tasks::context::DeviceTaskContext;
use crate::device_tasks::messages::{DeviceTaskMessage, Message};

const UUID: [u8; 6] = [1, 2, 3, 4, 5, 6];
const WAIT: Duration = Duration::from_secs(5);

struct Harness {
    commands: mpsc::Sender<DeviceTaskMessage>,
    messages: mpsc::Receiver<Message>,
    events: broadcast::Receiver<Event>,
    context: DeviceTaskContext,
    task: tokio::task::JoinHandle<()>,
}

fn device(capabilities: u8) -> Device {
    Device {
        uuid: UUID,
        transport: Transport::UsbMux,
        capabilities: DeviceCapabilities::new(capabilities),
        firmware_version: [0, 0, 0],
        events_transport: EventsTransport::Bluetooth,
        events_connected: false,
        product_id: 0,
        name: Device::name_bytes(""),
        netcore_release: 0,
    }
}

fn start_handler(capabilities: u8) -> Harness {
    start_handler_with_device(device(capabilities), None)
}

fn start_handler_with_device(device: Device, owner: Option<Arc<()>>) -> Harness {
    let (message_channel, messages) = mpsc::channel(16);
    let (event_sender, events) = broadcast::channel(16);
    let context = DeviceTaskContext {
        message_channel,
        screen_calibrations: Arc::new(ArcSwap::from_pointee(Default::default())),
        device_offsets: Arc::new(Mutex::new(HashMap::new())),
        device_shot_delays: Arc::new(RwLock::new(HashMap::new())),
        shot_delay_watch: Arc::new(RwLock::new(HashMap::new())),
        event_sender,
        direct_usb_handler_uuids: Arc::new(Mutex::new(HashSet::new())),
        bond_names: Arc::new(Mutex::new(HashMap::new())),
    };
    let (commands, control_rx) = mpsc::channel(16);
    let init = DeviceHandlerInit {
        connections: DeviceConnections {
            control_device: None,
            events_device: None,
        },
        device,
        control_rx,
        context: context.clone(),
        protocol_version: [0, 0, 0],
        hub_device: None,
        self_tx: commands.downgrade(),
    };
    let task = tokio::spawn(async move {
        let mut handler = DeviceHandler::start(init).await;
        handler.events_owner = owner;
        handler.run().await;
    });
    Harness {
        commands,
        messages,
        events,
        context,
        task,
    }
}

impl Harness {
    /// Messages are handled in order, so once this reply arrives everything sent before
    /// it has been handled.
    async fn barrier(&self) {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(DeviceTaskMessage::ReadRegister {
                port: 0,
                bank: 0,
                address: 0,
                reply,
            })
            .await
            .expect("handler is running");
        let result = tokio::time::timeout(WAIT, answer)
            .await
            .expect("handler answers")
            .expect("handler replies");
        assert!(result.is_err());
    }

    async fn next_message(&mut self) -> Message {
        tokio::time::timeout(WAIT, self.messages.recv())
            .await
            .expect("a message arrives")
            .expect("channel open")
    }

    async fn next_device_event(&mut self) -> DeviceEventKind {
        let event = tokio::time::timeout(WAIT, self.events.recv())
            .await
            .expect("an event arrives")
            .expect("channel open");
        let Event::DeviceEvent(DeviceEvent(device, kind)) = event else {
            panic!("expected a device event");
        };
        assert_eq!(device.uuid, UUID);
        kind
    }

    async fn finish(self) -> Device {
        let Self {
            commands,
            mut messages,
            task,
            ..
        } = self;
        drop(commands);
        tokio::time::timeout(WAIT, task)
            .await
            .expect("handler ends")
            .expect("handler does not panic");
        let Some(Message::Disconnect(device)) = messages.recv().await else {
            panic!("a handler that ends reports Disconnect");
        };
        device
    }
}

#[tokio::test]
async fn closing_the_control_channel_ends_the_handler_with_a_disconnect() {
    let harness = start_handler(DeviceCapabilities::CONTROL);
    let device = harness.finish().await;
    assert_eq!(device.uuid, UUID);
}

#[tokio::test]
async fn a_shot_delay_is_stored_and_reaches_the_watcher() {
    let harness = start_handler(0);
    harness
        .commands
        .send(DeviceTaskMessage::SetShotDelay(120))
        .await
        .unwrap();
    harness.barrier().await;

    assert_eq!(
        harness.context.device_shot_delays.read().await.get(&UUID),
        Some(&120)
    );
    let watchers = harness.context.shot_delay_watch.read().await;
    assert_eq!(*watchers.get(&UUID).unwrap().borrow(), 120);
}

#[tokio::test]
async fn unknown_config_kind_is_refused() {
    let harness = start_handler(0);
    let (reply, answer) = oneshot::channel();
    harness
        .commands
        .send(DeviceTaskMessage::ReadConfig { kind: 9, reply })
        .await
        .unwrap();
    assert!(answer.await.unwrap().is_err());
}

#[tokio::test]
async fn net_core_release_is_published_once_per_change() {
    let mut harness = start_handler(0);
    harness
        .commands
        .send(DeviceTaskMessage::SetNetCoreRelease(7))
        .await
        .unwrap();
    let Message::UpdateDevice(updated) = harness.next_message().await else {
        panic!("expected UpdateDevice");
    };
    assert_eq!(updated.netcore_release, 7);
    assert!(
        updated
            .capabilities
            .contains(DeviceCapabilities::NETCORE_KNOWN)
    );

    // Reporting the same release again is not news.
    harness
        .commands
        .send(DeviceTaskMessage::SetNetCoreRelease(7))
        .await
        .unwrap();
    harness.barrier().await;
    assert!(harness.messages.try_recv().is_err());
}

#[tokio::test]
async fn losing_the_events_device_of_a_unit_without_control_ends_the_handler() {
    let harness = start_handler(DeviceCapabilities::EVENTS);
    harness
        .commands
        .send(DeviceTaskMessage::RemoveEventsDevice(Arc::new(())))
        .await
        .unwrap();
    // The handler ends on its own while the command channel is still open.
    let Harness {
        task, mut messages, ..
    } = harness;
    tokio::time::timeout(WAIT, task)
        .await
        .expect("handler ends")
        .unwrap();
    let Some(Message::Disconnect(device)) = messages.recv().await else {
        panic!("a handler that ends reports Disconnect");
    };
    assert!(!device.capabilities.contains(DeviceCapabilities::EVENTS));
    assert!(!device.events_connected);
}

#[tokio::test]
async fn losing_an_ignored_mux_does_not_disconnect_wired_events() {
    let mut connected = device(DeviceCapabilities::CONTROL | DeviceCapabilities::EVENTS);
    connected.events_transport = EventsTransport::Wired;
    connected.events_connected = true;
    let harness = start_handler_with_device(connected, None);
    harness
        .commands
        .send(DeviceTaskMessage::RemoveEventsDevice(Arc::new(())))
        .await
        .unwrap();
    harness.barrier().await;
    let disconnected = harness.finish().await;
    assert!(
        disconnected
            .capabilities
            .contains(DeviceCapabilities::EVENTS)
    );
    assert!(disconnected.events_connected);
}

#[tokio::test]
async fn departing_old_dongle_cannot_remove_replacement_events() {
    let owner = Arc::new(());
    let harness =
        start_handler_with_device(device(DeviceCapabilities::EVENTS), Some(Arc::clone(&owner)));
    harness
        .commands
        .send(DeviceTaskMessage::RemoveEventsDevice(Arc::new(())))
        .await
        .unwrap();
    harness.barrier().await;
    harness
        .commands
        .send(DeviceTaskMessage::RemoveEventsDevice(owner))
        .await
        .unwrap();
    let Harness {
        task, mut messages, ..
    } = harness;
    tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
    let Some(Message::Disconnect(disconnected)) = messages.recv().await else {
        panic!("current dongle departure must disconnect the unit");
    };
    assert!(
        !disconnected
            .capabilities
            .contains(DeviceCapabilities::EVENTS)
    );
}

#[tokio::test]
async fn zeroing_without_a_pose_reports_failure() {
    let mut harness = start_handler(0);
    harness
        .commands
        .send(DeviceTaskMessage::Zero(
            Translation3::identity(),
            Point2::new(0.5, 0.5),
            None,
        ))
        .await
        .unwrap();
    assert!(matches!(
        harness.next_device_event().await,
        DeviceEventKind::ZeroResult(false)
    ));
    assert!(harness.context.device_offsets.lock().await.is_empty());
}

#[tokio::test]
async fn resetting_zero_forgets_the_stored_offset() {
    let mut harness = start_handler(0);
    harness
        .context
        .device_offsets
        .lock()
        .await
        .insert(UUID, Isometry3::identity());
    harness
        .commands
        .send(DeviceTaskMessage::ResetZero)
        .await
        .unwrap();
    assert!(matches!(
        harness.next_device_event().await,
        DeviceEventKind::ZeroResult(true)
    ));
    assert!(harness.context.device_offsets.lock().await.is_empty());
}
