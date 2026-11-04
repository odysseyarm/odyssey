//! The handler's message loop, driven without hardware: a handler with no connections
//! still serves every message that does not need a device, and in-process fake links
//! stand in for USB and BLE where a test needs reports to flow.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use nalgebra::{Isometry3, Point2, Translation3};
use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};
use odyssey_hub_common::events::{DeviceEvent, DeviceEventKind, Event};
use odyssey_usb::device::{PacketTransport, VmDevice};
use odyssey_usb::packets::vm::{ImpactReport, Packet, PacketData, StreamUpdateAction};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, oneshot};

use super::{DeviceConnections, DeviceHandler, DeviceHandlerInit};
use crate::device_tasks::context::DeviceTaskContext;
use crate::device_tasks::messages::{DeviceTaskMessage, Message};
use crate::device_tasks::transport_selection::TransportSelection;

const UUID: [u8; 6] = [1, 2, 3, 4, 5, 6];
const WAIT: Duration = Duration::from_secs(5);
/// Firmware from this protocol on picks its own transport.
const AUTO_PROTOCOL: [u16; 3] = [0, 1, 6];
/// The unit stamps reports with whatever id the last Enable carried, which may have come
/// over the other link, so reports here carry an id this link never issued.
const FOREIGN_REPORT_ID: u8 = 254;
/// Id of a host packet that wants no reply, such as a stream Disable.
const NO_REPLY_ID: u8 = 255;

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

/// One link to a simulated unit. The unit acknowledges every request, so config and
/// version reads fail fast instead of timing out, and stream Enables need no answer.
struct FakeLink {
    device: VmDevice,
    /// Everything the host sent over this link, in order.
    sent: mpsc::UnboundedReceiver<Packet>,
    to_host: mpsc::Sender<Packet>,
}

impl FakeLink {
    fn new() -> Self {
        let (writer, mut written) = mpsc::channel::<Packet>(64);
        let (to_host, incoming) = mpsc::channel::<Packet>(64);
        let (log, sent) = mpsc::unbounded_channel();
        let acks = to_host.clone();
        tokio::spawn(async move {
            while let Some(packet) = written.recv().await {
                if packet.id != NO_REPLY_ID {
                    let _ = acks
                        .send(Packet {
                            id: packet.id,
                            data: PacketData::Ack(),
                        })
                        .await;
                }
                let _ = log.send(packet);
            }
        });
        Self {
            device: VmDevice::from_transport(
                PacketTransport::from_channels(writer, incoming),
                None,
            ),
            sent,
            to_host,
        }
    }

    /// A USB link whose cable was pulled after the probe: every send fails at once.
    fn unplugged() -> VmDevice {
        let (writer, _) = mpsc::channel::<Packet>(1);
        let (_, incoming) = mpsc::channel::<Packet>(1);
        VmDevice::from_transport(PacketTransport::from_channels(writer, incoming), None)
    }

    /// Wait for the host to send a packet that `matches` over this link.
    async fn receives(&mut self, matches: impl Fn(&PacketData) -> bool) -> bool {
        tokio::time::timeout(WAIT, async {
            while let Some(packet) = self.sent.recv().await {
                if matches(&packet.data) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    }

    async fn report_impact(&self, timestamp: u32) {
        self.to_host
            .send(Packet {
                id: FOREIGN_REPORT_ID,
                data: PacketData::ImpactReport(ImpactReport { timestamp }),
            })
            .await
            .expect("fake unit is running");
    }

    /// The stream updates sent since the last call, in order.
    fn stream_actions(&mut self) -> Vec<StreamUpdateAction> {
        let mut actions = Vec::new();
        while let Ok(packet) = self.sent.try_recv() {
            if let PacketData::StreamUpdate(update) = packet.data {
                actions.push(update.action);
            }
        }
        actions
    }
}

fn start_handler(capabilities: u8) -> Harness {
    start_handler_with_device(device(capabilities), None)
}

fn start_handler_with_device(device: Device, owner: Option<Arc<()>>) -> Harness {
    start_handler_with_links(device, owner, None, None)
}

fn start_handler_with_links(
    device: Device,
    owner: Option<Arc<()>>,
    control_device: Option<VmDevice>,
    events_device: Option<VmDevice>,
) -> Harness {
    let (message_channel, messages) = mpsc::channel(16);
    let (event_sender, events) = broadcast::channel(64);
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
            control_device,
            events_device,
        },
        device,
        control_rx,
        context: context.clone(),
        protocol_version: AUTO_PROTOCOL,
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

/// A unit with automatic transport that is connected over BLE only, streaming.
fn start_auto_unit_over_ble(ble: &FakeLink) -> Harness {
    let mut connected = device(DeviceCapabilities::EVENTS | DeviceCapabilities::AUTO_TRANSPORT);
    connected.events_connected = true;
    start_handler_with_links(connected, None, None, Some(ble.device.clone()))
}

impl Harness {
    /// Messages are handled in order, so once this reply arrives everything sent before
    /// it has been handled. An unknown config kind is refused without touching a link.
    async fn barrier(&self) {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(DeviceTaskMessage::ReadConfig { kind: 9, reply })
            .await
            .expect("handler is running");
        let result = tokio::time::timeout(WAIT, answer)
            .await
            .expect("handler answers")
            .expect("handler replies");
        assert!(result.is_err());
    }

    async fn send(&self, message: DeviceTaskMessage) {
        self.commands
            .send(message)
            .await
            .expect("handler is running");
    }

    async fn plug_usb(&self, usb: &FakeLink, transport_selection: TransportSelection) {
        self.send(DeviceTaskMessage::AddControlDevice {
            device: usb.device.clone(),
            dfu: false,
            events_protocol_version: AUTO_PROTOCOL,
            transport_selection,
        })
        .await;
    }

    /// The device as last published to the device list.
    async fn next_update(&mut self) -> Device {
        match self.next_message().await {
            Message::UpdateDevice(device) => device,
            Message::Connect(..) => panic!("the device must not be listed again"),
            Message::Disconnect(..) => panic!("the device must not leave the list"),
            _ => panic!("expected UpdateDevice"),
        }
    }

    /// Wait for the impact report with `timestamp` to come out as an event.
    async fn expect_impact(&mut self, timestamp: u32) {
        loop {
            if let DeviceEventKind::ImpactEvent(impact) = self.next_device_event().await
                && impact.timestamp == timestamp
            {
                return;
            }
        }
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

fn stops_a_stream(action: &StreamUpdateAction) -> bool {
    matches!(
        action,
        StreamUpdateAction::Disable | StreamUpdateAction::DisableAll
    )
}

#[tokio::test]
async fn plugging_usb_into_a_unit_on_ble_keeps_one_device_and_moves_events_to_usb() {
    let ble = FakeLink::new();
    let mut usb = FakeLink::new();
    let mut harness = start_auto_unit_over_ble(&ble);

    harness.plug_usb(&usb, TransportSelection::Automatic).await;
    let plugged = harness.next_update().await;
    harness.barrier().await;

    assert_eq!(plugged.transport, Transport::Usb);
    assert_eq!(plugged.events_transport, EventsTransport::Wired);
    assert!(plugged.events_connected);
    assert!(plugged.capabilities.contains(
        DeviceCapabilities::CONTROL
            | DeviceCapabilities::EVENTS
            | DeviceCapabilities::AUTO_TRANSPORT
    ));
    assert!(
        harness.messages.try_recv().is_err(),
        "plugging in only updates the listed device"
    );
    assert!(
        usb.receives(|data| matches!(
            data,
            PacketData::StreamUpdate(update) if update.action == StreamUpdateAction::Enable
        ))
        .await,
        "streams are subscribed on the new link"
    );
}

#[tokio::test]
async fn reports_from_both_links_are_forwarded_while_usb_carries_the_events() {
    let mut ble = FakeLink::new();
    let usb = FakeLink::new();
    let mut harness = start_auto_unit_over_ble(&ble);
    harness.plug_usb(&usb, TransportSelection::Automatic).await;
    harness.next_update().await;

    // The firmware moves to BLE the moment the cable goes, before the USB scan notices.
    ble.report_impact(1).await;
    harness.expect_impact(1).await;
    usb.report_impact(2).await;
    harness.expect_impact(2).await;

    assert!(
        !ble.stream_actions().iter().any(stops_a_stream),
        "the standby link's subscriptions are the unit's, and must stay"
    );
}

#[tokio::test]
async fn pulling_usb_keeps_the_device_listed_with_events_over_ble() {
    let mut ble = FakeLink::new();
    let mut usb = FakeLink::new();
    let mut harness = start_auto_unit_over_ble(&ble);
    harness.plug_usb(&usb, TransportSelection::Automatic).await;
    harness.next_update().await;

    harness.send(DeviceTaskMessage::RemoveControlDevice).await;
    let unplugged = harness.next_update().await;

    assert_eq!(unplugged.transport, Transport::UsbMux);
    assert_eq!(unplugged.events_transport, EventsTransport::Bluetooth);
    assert!(unplugged.events_connected);
    assert!(!unplugged.capabilities.contains(DeviceCapabilities::CONTROL));
    assert!(unplugged.capabilities.contains(DeviceCapabilities::EVENTS));
    ble.report_impact(3).await;
    harness.expect_impact(3).await;
    assert!(!ble.stream_actions().iter().any(stops_a_stream));
    assert!(
        !usb.stream_actions().iter().any(stops_a_stream),
        "a stop sent down the dropped link would reach the shared subscriptions"
    );
    harness.barrier().await;
}

#[tokio::test]
async fn auto_transport_firmware_is_not_asked_to_switch_transport() {
    let ble = FakeLink::new();
    let usb = FakeLink::new();
    let mut harness = start_auto_unit_over_ble(&ble);
    harness.plug_usb(&usb, TransportSelection::Automatic).await;
    harness.next_update().await;

    // The fake USB link has no control endpoint, so a request that reached it would fail.
    let (reply, answer) = oneshot::channel();
    harness
        .send(DeviceTaskMessage::SetTransportMode {
            usb_mode: false,
            reply,
        })
        .await;
    assert_eq!(answer.await.unwrap(), Ok(true));
    harness.barrier().await;
    assert!(harness.messages.try_recv().is_err());
}

#[tokio::test]
async fn legacy_firmware_keeps_manual_transport_when_usb_is_plugged_in() {
    let ble = FakeLink::new();
    let usb = FakeLink::new();
    let mut connected = device(DeviceCapabilities::EVENTS);
    connected.events_connected = true;
    let mut harness = start_handler_with_links(connected, None, None, Some(ble.device.clone()));

    harness.plug_usb(&usb, TransportSelection::Manual).await;
    let plugged = harness.next_update().await;

    // The transport mode is unreadable on the fake link, so the events stay where they are.
    assert_eq!(plugged.transport, Transport::Usb);
    assert_eq!(plugged.events_transport, EventsTransport::Bluetooth);
    assert!(
        !plugged
            .capabilities
            .contains(DeviceCapabilities::AUTO_TRANSPORT)
    );
    ble.report_impact(4).await;
    harness.expect_impact(4).await;

    // The old switch reboots the unit, so the handler ends whatever the outcome.
    let (reply, answer) = oneshot::channel();
    harness
        .send(DeviceTaskMessage::SetTransportMode {
            usb_mode: true,
            reply,
        })
        .await;
    assert!(
        answer.await.unwrap().is_err(),
        "the request goes to the unit's control endpoint"
    );
    let Harness {
        task, mut messages, ..
    } = harness;
    tokio::time::timeout(WAIT, task)
        .await
        .expect("handler ends")
        .unwrap();
    assert!(matches!(
        messages.recv().await,
        Some(Message::Disconnect(_))
    ));
}

#[tokio::test]
async fn legacy_firmware_streaming_over_usb_leaves_the_list_when_unplugged() {
    let usb = FakeLink::new();
    let mut wired = device(DeviceCapabilities::CONTROL | DeviceCapabilities::EVENTS);
    wired.transport = Transport::Usb;
    wired.events_transport = EventsTransport::Wired;
    wired.events_connected = true;
    let harness = start_handler_with_links(
        wired,
        None,
        Some(usb.device.clone()),
        Some(usb.device.clone()),
    );

    harness.send(DeviceTaskMessage::RemoveControlDevice).await;
    let Harness {
        task, mut messages, ..
    } = harness;
    tokio::time::timeout(WAIT, task)
        .await
        .expect("handler ends")
        .unwrap();
    assert!(matches!(
        messages.recv().await,
        Some(Message::Disconnect(_))
    ));
}

#[tokio::test]
async fn a_request_that_fails_over_a_pulled_cable_goes_over_ble() {
    let mut ble = FakeLink::new();
    let mut harness = start_auto_unit_over_ble(&ble);
    harness
        .send(DeviceTaskMessage::AddControlDevice {
            device: FakeLink::unplugged(),
            dfu: false,
            events_protocol_version: AUTO_PROTOCOL,
            transport_selection: TransportSelection::Automatic,
        })
        .await;
    harness.next_update().await;
    ble.stream_actions();

    let (reply, answer) = oneshot::channel();
    harness
        .send(DeviceTaskMessage::WriteConfig {
            kind: 0,
            value: 42,
            reply,
        })
        .await;

    assert_eq!(answer.await.unwrap(), Ok(()));
    assert!(
        ble.receives(|data| matches!(data, PacketData::WriteConfig(_)))
            .await
    );
}
