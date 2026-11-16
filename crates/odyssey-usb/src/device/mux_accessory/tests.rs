use odyssey_protocol::List;
use odyssey_protocol::accessory::{self, Command, Completion, Event, Profile, Reply, Request};
use odyssey_protocol::control::usb_mux::UsbMuxCtrlMsg;
use tokio::sync::{broadcast, mpsc};

use super::{EVENT_CAPACITY, REQUEST_TIMEOUT};
use crate::Error;
use crate::device::MuxDevice;
use crate::device::mux_transport::{DevicePacketRoute, register_route};
use crate::packets::mux::{DevicePacket, MuxMsg, Version};
use crate::packets::vm::{Packet, PacketData};

struct Harness {
    device: MuxDevice,
    sent: mpsc::Receiver<MuxMsg>,
    incoming: mpsc::Sender<MuxMsg>,
}

fn harness() -> Harness {
    let (writer, sent) = mpsc::channel(64);
    let (incoming, reader) = mpsc::channel(128);
    let (control_writer, _control_sent) = mpsc::channel::<UsbMuxCtrlMsg>(1);
    let (_control_incoming, control_reader) = mpsc::channel(1);
    Harness {
        device: MuxDevice::from_simulated_channels(writer, reader, control_writer, control_reader),
        sent,
        incoming,
    }
}

async fn ready() -> Harness {
    ready_with(accessory::MIN_MUX_PROTOCOL).await
}

async fn ready_with(protocol_semver: [u16; 3]) -> Harness {
    let mut harness = harness();
    let reading = tokio::spawn({
        let device = harness.device.clone();
        async move { device.read_version().await }
    });
    assert!(matches!(
        harness.sent.recv().await,
        Some(MuxMsg::ReadVersion())
    ));
    harness
        .incoming
        .send(MuxMsg::ReadVersionResponse(Version {
            protocol_semver,
            firmware_semver: [1, 2, 3],
        }))
        .await
        .unwrap();
    assert_eq!(reading.await.unwrap().unwrap().firmware_semver, [1, 2, 3]);
    harness
}

async fn request(sent: &mut mpsc::Receiver<MuxMsg>) -> Request {
    match sent.recv().await.unwrap() {
        MuxMsg::AccessoryRequest(request) => request,
        other => panic!("expected accessory request, got {other:?}"),
    }
}

async fn complete(
    incoming: &mpsc::Sender<MuxMsg>,
    id: u32,
    result: Result<Reply, accessory::Error>,
) {
    incoming
        .send(MuxMsg::AccessoryCompletion(Completion { id, result }))
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn old_firmware_fails_promptly_and_clones_share_version_request() {
    let mut harness = harness();
    let reading = tokio::spawn({
        let device = harness.device.clone();
        async move { device.read_version().await }
    });
    let pending = tokio::spawn({
        let device = harness.device.clone();
        async move { device.accessory_request(Command::Capabilities()).await }
    });
    assert!(matches!(
        harness.sent.recv().await,
        Some(MuxMsg::ReadVersion())
    ));
    harness
        .incoming
        .send(MuxMsg::ReadVersionResponse(Version {
            protocol_semver: [0, 2, 0],
            firmware_semver: [4, 5, 6],
        }))
        .await
        .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_millis(1), pending)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(Error::AccessoryUnsupported)));
    assert_eq!(reading.await.unwrap().unwrap().protocol_semver, [0, 2, 0]);
    assert!(matches!(
        harness
            .device
            .accessory_request(Command::Scan { enabled: true })
            .await,
        Err(Error::AccessoryUnsupported)
    ));
    assert!(matches!(
        harness.sent.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn unrelated_completion_cannot_finish_request_and_firmware_failure_is_preserved() {
    let mut harness = ready().await;
    let mut events = harness.device.subscribe_accessory_events();
    let pending = tokio::spawn({
        let device = harness.device.clone();
        async move { device.accessory_request(Command::Capabilities()).await }
    });
    let sent = request(&mut harness.sent).await;
    complete(&harness.incoming, sent.id + 1, Ok(Reply::Done())).await;
    harness
        .incoming
        .send(MuxMsg::AccessoryEvent(Event::Disconnected {
            address: [1; 6],
        }))
        .await
        .unwrap();
    assert_eq!(
        events.recv().await.unwrap(),
        Event::Disconnected { address: [1; 6] }
    );
    assert!(!pending.is_finished());
    complete(&harness.incoming, sent.id, Err(accessory::Error::Security)).await;
    assert!(matches!(
        pending.await.unwrap(),
        Err(Error::Accessory(accessory::Error::Security))
    ));
}

#[tokio::test(start_paused = true)]
async fn timeout_releases_capacity_without_reusing_late_completion_id() {
    let mut harness = ready().await;
    let pending = tokio::spawn({
        let device = harness.device.clone();
        async move { device.accessory_request(Command::Capabilities()).await }
    });
    let old = request(&mut harness.sent).await;
    tokio::time::advance(REQUEST_TIMEOUT).await;
    assert!(
        matches!(pending.await.unwrap(), Err(Error::Timeout(timeout)) if timeout == REQUEST_TIMEOUT)
    );
    let next = tokio::spawn({
        let device = harness.device.clone();
        async move {
            device
                .accessory_request(Command::Scan { enabled: false })
                .await
        }
    });
    let sent = request(&mut harness.sent).await;
    assert_ne!(sent.id, old.id);
    complete(&harness.incoming, old.id, Ok(Reply::Done())).await;
    complete(&harness.incoming, sent.id, Err(accessory::Error::Gatt)).await;
    assert!(matches!(
        next.await.unwrap(),
        Err(Error::Accessory(accessory::Error::Gatt))
    ));
}

#[tokio::test(start_paused = true)]
async fn unplug_wakes_requests_events_and_future_clone_requests() {
    let mut harness = ready().await;
    let clone = harness.device.clone();
    let mut events = clone.subscribe_accessory_events();
    let pending = tokio::spawn({
        let device = clone.clone();
        async move { device.accessory_request(Command::Capabilities()).await }
    });
    request(&mut harness.sent).await;
    drop(harness.incoming);
    let result = tokio::time::timeout(std::time::Duration::from_millis(1), pending)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(Error::Closed)));
    assert!(matches!(
        events.recv().await,
        Err(broadcast::error::RecvError::Closed)
    ));
    assert!(matches!(
        clone.accessory_request(Command::Capabilities()).await,
        Err(Error::Closed)
    ));
    assert!(matches!(
        clone.subscribe_accessory_events().recv().await,
        Err(broadcast::error::RecvError::Closed)
    ));
}

#[tokio::test(start_paused = true)]
async fn unplug_wakes_pending_version_gate() {
    let mut harness = harness();
    let pending = tokio::spawn({
        let device = harness.device.clone();
        async move { device.accessory_request(Command::Capabilities()).await }
    });
    assert!(matches!(
        harness.sent.recv().await,
        Some(MuxMsg::ReadVersion())
    ));
    drop(harness.incoming);
    let result = tokio::time::timeout(std::time::Duration::from_millis(1), pending)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(Error::Closed)));
}

#[tokio::test]
async fn shared_capacity_bounds_pending_operations_without_serializing_marker_writes() {
    let mut harness = ready().await;
    let mut pending = Vec::new();
    let mut ids = Vec::new();
    for index in 0..accessory::MAX_PENDING_REQUESTS {
        pending.push(tokio::spawn({
            let device = harness.device.clone();
            async move {
                device
                    .accessory_request(Command::Connect {
                        address: [index as u8; 6],
                        profile: Profile::DryFireMag,
                        enroll: false,
                    })
                    .await
            }
        }));
        ids.push(request(&mut harness.sent).await.id);
    }
    assert!(matches!(
        harness
            .device
            .accessory_request(Command::Scan { enabled: true })
            .await,
        Err(Error::Busy)
    ));
    let cancelled = pending.remove(0);
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    complete(&harness.incoming, ids[0], Ok(Reply::Done())).await;
    let marker = tokio::spawn({
        let device = harness.device.clone();
        async move {
            device
                .accessory_request(Command::MarkerDemand {
                    address: [42; 6],
                    active: true,
                })
                .await
        }
    });
    let sent = request(&mut harness.sent).await;
    assert_eq!(
        sent.command,
        Command::MarkerDemand {
            address: [42; 6],
            active: true
        }
    );
    complete(&harness.incoming, sent.id, Ok(Reply::Done())).await;
    assert_eq!(marker.await.unwrap().unwrap(), Reply::Done());
    drop(harness.incoming);
    for task in pending {
        assert!(matches!(task.await.unwrap(), Err(Error::Closed)));
    }
}

#[tokio::test]
async fn clone_keeps_link_alive_after_original_drop_and_ignores_cancelled_completion() {
    let mut harness = ready().await;
    let cancelled = tokio::spawn({
        let device = harness.device.clone();
        async move { device.accessory_request(Command::Capabilities()).await }
    });
    let old = request(&mut harness.sent).await;
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    let clone = harness.device.clone();
    drop(harness.device);
    let pending =
        tokio::spawn(async move { clone.accessory_request(Command::Capabilities()).await });
    let sent = request(&mut harness.sent).await;
    assert_ne!(sent.id, old.id);
    complete(
        &harness.incoming,
        old.id,
        Err(accessory::Error::Disconnected),
    )
    .await;
    let capabilities = accessory::Capabilities {
        total_capacity: 6,
        tracker_capacity: 3,
        accessory_capacity: 3,
        connected_trackers: 2,
        connected_accessories: 1,
        connecting_accessories: 1,
    };
    complete(
        &harness.incoming,
        sent.id,
        Ok(Reply::Capabilities(capabilities)),
    )
    .await;
    assert_eq!(
        pending.await.unwrap().unwrap(),
        Reply::Capabilities(capabilities)
    );
}

#[tokio::test]
async fn slow_or_dropped_event_subscribers_do_not_pin_tracker_or_completion_routing() {
    let mut harness = ready().await;
    let mut events = harness.device.subscribe_accessory_events();
    let address = [7; 6];
    let (packets, mut tracker) = mpsc::channel(1);
    register_route(
        &harness.device.device_packets_tx,
        address,
        DevicePacketRoute {
            sender: packets,
            cancel: tokio_util::sync::CancellationToken::new(),
        },
    );
    for _ in 0..=EVENT_CAPACITY {
        harness
            .incoming
            .send(MuxMsg::AccessoryEvent(Event::Disconnected { address }))
            .await
            .unwrap();
    }
    harness
        .incoming
        .send(MuxMsg::DevicePacket(DevicePacket {
            dev: address,
            pkt: Packet {
                id: 23,
                data: PacketData::Ack(),
            },
        }))
        .await
        .unwrap();
    assert!(matches!(
        tracker.recv().await,
        Some(Packet {
            id: 23,
            data: PacketData::Ack()
        })
    ));
    assert!(matches!(
        events.recv().await,
        Err(broadcast::error::RecvError::Lagged(1))
    ));
    drop(events);
    harness
        .incoming
        .send(MuxMsg::AccessoryEvent(Event::Disconnected { address }))
        .await
        .unwrap();
    let pending = tokio::spawn({
        let device = harness.device.clone();
        async move { device.accessory_request(Command::Capabilities()).await }
    });
    let sent = request(&mut harness.sent).await;
    complete(&harness.incoming, sent.id, Ok(Reply::Done())).await;
    assert_eq!(pending.await.unwrap().unwrap(), Reply::Done());
}

#[test]
fn malformed_notification_payloads_are_rejected_at_deserialization_boundary() {
    let encoded =
        |data: Vec<u8>| postcard::to_stdvec(&(3u8, [1u8; 6], Profile::DryFireMag, data)).unwrap();
    let valid = encoded(vec![1; accessory::MAX_NOTIFICATION]);
    assert!(
        matches!(postcard::from_bytes::<Event>(&valid), Ok(Event::Notification { data, .. })
        if data[..] == [1; accessory::MAX_NOTIFICATION])
    );
    postcard::from_bytes::<Event>(&valid[..valid.len() - 1]).unwrap_err();
    postcard::from_bytes::<Event>(&encoded(vec![1; accessory::MAX_NOTIFICATION + 1])).unwrap_err();
    let mut malformed = serde_json::json!({ "Notification": {
        "address": [1, 1, 1, 1, 1, 1], "profile": "DryFireMag", "data": [256]
    }});
    serde_json::from_value::<Event>(malformed.clone()).unwrap_err();
    malformed["Notification"]["data"] = serde_json::json!(vec![1; accessory::MAX_NOTIFICATION + 1]);
    serde_json::from_value::<Event>(malformed).unwrap_err();
}

#[test]
fn mux_wire_discriminants_preserve_existing_tracker_firmware_compatibility() {
    let packet = Packet {
        id: 3,
        data: PacketData::Ack(),
    };
    let messages = [
        (MuxMsg::DevicesSnapshot(List::default()), 0),
        (
            MuxMsg::DevicePacket(DevicePacket {
                dev: [1; 6],
                pkt: packet.clone(),
            }),
            1,
        ),
        (MuxMsg::RequestDevices(), 2),
        (
            MuxMsg::SendTo(crate::packets::mux::SendTo {
                dev: [1; 6],
                pkt: packet,
            }),
            3,
        ),
        (MuxMsg::ReadVersion(), 4),
        (
            MuxMsg::ReadVersionResponse(Version {
                protocol_semver: [0, 2, 0],
                firmware_semver: [1, 2, 3],
            }),
            5,
        ),
        (MuxMsg::SubscribeDeviceList(), 6),
        (MuxMsg::UnsubscribeDeviceList(), 7),
        (
            MuxMsg::AccessoryRequest(Request {
                id: 1,
                command: Command::Capabilities(),
            }),
            8,
        ),
        (
            MuxMsg::AccessoryCompletion(Completion {
                id: 1,
                result: Ok(Reply::Done()),
            }),
            9,
        ),
        (
            MuxMsg::AccessoryEvent(Event::Disconnected { address: [1; 6] }),
            10,
        ),
    ];
    for (message, discriminant) in messages {
        assert_eq!(
            postcard::to_stdvec(&message).unwrap()[0],
            discriminant,
            "{message:?}"
        );
    }
    let mut addresses = heapless::Vec::new();
    addresses.push([1, 2, 3, 4, 5, 6]).unwrap();
    assert_eq!(
        postcard::to_stdvec(&MuxMsg::DevicesSnapshot(addresses.into())).unwrap(),
        [0, 1, 1, 2, 3, 4, 5, 6]
    );
    assert_eq!(postcard::to_stdvec(&MuxMsg::RequestDevices()).unwrap(), [2]);
}

#[test]
fn accessory_event_wire_indices_remain_compatible_with_old_dongles() {
    let address = [1; 6];
    let events = [
        Event::Discovered {
            address,
            profile: Profile::MarkerControlBoard,
            name: List::default(),
        },
        Event::Connected {
            address,
            profile: Profile::MarkerControlBoard,
        },
        Event::Disconnected { address },
        Event::Notification {
            address,
            profile: Profile::DryFireMag,
            data: List::default(),
        },
        Event::Error {
            address,
            error: accessory::Error::Security,
        },
        Event::PairingWindow {
            address,
            seconds_remaining: 30,
        },
    ];
    for (index, event) in events.into_iter().enumerate() {
        let bytes = postcard::to_stdvec(&event).unwrap();
        assert_eq!(bytes[0], index as u8);
        assert_eq!(postcard::from_bytes::<Event>(&bytes).unwrap(), event);
    }
    let bytes = [10, 5, 1, 1, 1, 1, 1, 1, 30];
    assert!(matches!(
        postcard::from_bytes::<MuxMsg>(&bytes).unwrap(),
        MuxMsg::AccessoryEvent(Event::PairingWindow {
            address: [1, 1, 1, 1, 1, 1],
            seconds_remaining: 30
        })
    ));
}

#[test]
fn marker_mode_command_is_appended_and_roundtrips() {
    let demand = Command::MarkerDemand {
        address: [1; 6],
        active: true,
    };
    let mode = Command::MarkerMode {
        address: [1; 6],
        mode: odyssey_protocol::marker_control::Mode::Auto,
    };
    assert_eq!(postcard::to_stdvec(&demand).unwrap()[0], 4);
    let bytes = postcard::to_stdvec(&mode).unwrap();
    assert_eq!(bytes, [5, 1, 1, 1, 1, 1, 1, 0]);
    assert_eq!(postcard::from_bytes::<Command>(&bytes).unwrap(), mode);
}

#[tokio::test]
async fn marker_mode_requires_mux_protocol_that_can_decode_it() {
    let command = Command::MarkerMode {
        address: [1; 6],
        mode: odyssey_protocol::marker_control::Mode::Auto,
    };
    let mut old = ready().await;
    assert!(matches!(
        old.device.accessory_request(command).await,
        Err(Error::MarkerModeUnsupported)
    ));
    assert!(matches!(
        old.sent.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    let mut current = ready_with(accessory::MARKER_MODE_MUX_PROTOCOL).await;
    let pending = tokio::spawn({
        let device = current.device.clone();
        async move { device.accessory_request(command).await }
    });
    let sent = request(&mut current.sent).await;
    assert_eq!(sent.command, command);
    complete(&current.incoming, sent.id, Ok(Reply::Done())).await;
    assert_eq!(pending.await.unwrap().unwrap(), Reply::Done());
}
