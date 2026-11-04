use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use super::VmDevice;
use crate::device::packet_transport::PacketTransport;
use crate::device::response_slots::{ABANDONED_ID_QUARANTINE, ResponseChannel};
use crate::packets::vm::{Packet, PacketData};

struct Harness {
    device: VmDevice,
    /// Packets the device wrote to the transport.
    sent: mpsc::Receiver<Packet>,
    /// Feeds packets "from the device"; dropping it closes the transport.
    incoming_tx: mpsc::Sender<Packet>,
}

fn harness() -> Harness {
    let (writer, sent) = mpsc::channel::<Packet>(64);
    let (incoming_tx, incoming_rx) = mpsc::channel::<Packet>(8);
    let transport = PacketTransport {
        writer,
        incoming_rx,
        cancel: Arc::new(tokio_util::sync::CancellationToken::new()),
    };
    Harness {
        device: VmDevice::from_transport(transport, None),
        sent,
        incoming_tx,
    }
}

#[tokio::test]
async fn pending_request_fails_fast_when_transport_closes() {
    let Harness {
        device,
        mut sent,
        incoming_tx,
    } = harness();
    let pending = tokio::spawn({
        let device = device.clone();
        async move { device.request(PacketData::ReadVersion()).await }
    });
    sent.recv().await.expect("request was written");

    // The USB reader exiting drops the only sender of the incoming stream.
    drop(incoming_tx);

    // Far below REQUEST_TIMEOUT: it must be the closed transport, not the timeout.
    let err = tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .expect("request hung after transport closed")
        .unwrap()
        .unwrap_err();
    assert!(!err.to_string().contains("timed out"), "{err}");
}

#[tokio::test(start_paused = true)]
async fn retry_resends_once_per_silent_attempt_then_gives_up() {
    let Harness {
        device,
        mut sent,
        incoming_tx: _keep_open,
    } = harness();

    let err = device
        .request_with_retry(PacketData::ReadVersion(), Duration::from_secs(2), 3)
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("no response after 3 attempts"),
        "{err}"
    );
    let mut writes = 0;
    while sent.try_recv().is_ok() {
        writes += 1;
    }
    assert_eq!(writes, 3);
}

fn slot_is_registered(device: &VmDevice, id: u8) -> bool {
    let state = device.thread_state.upgrade().unwrap();
    let chans = state.response_channels.lock();
    matches!(chans[usize::from(id)], ResponseChannel::Oneshot(..))
}

#[tokio::test]
async fn stale_slot_does_not_clear_request_that_reused_its_id() {
    let Harness {
        device,
        incoming_tx: _open,
        ..
    } = harness();
    let (stale, stale_rx) = device.get_oneshot_slot().unwrap();
    let id = stale.id;

    // The dispatcher took the sender to deliver the reply, freeing the id...
    let state = device.thread_state.upgrade().unwrap();
    state.response_channels.lock()[usize::from(id)] = ResponseChannel::None;
    // ...a newer request reuses it...
    let (newer, _newer_rx) = device.get_oneshot_slot().unwrap();
    assert_eq!(newer.id, id);

    // ...and only then the original request is cancelled (timeout).
    drop(stale);
    drop(stale_rx);

    assert!(slot_is_registered(&device, id), "newer request was cleared");
}

#[tokio::test]
async fn cancelled_request_releases_its_own_slot() {
    let Harness {
        device,
        incoming_tx: _open,
        ..
    } = harness();
    let (slot, _rx_still_alive) = device.get_oneshot_slot().unwrap();
    let id = slot.id;

    drop(slot);

    assert!(!slot_is_registered(&device, id), "slot leaked");
}

/// Reserve a slot and mark its request as sent, then drop it as a timeout would.
fn time_out_request(device: &VmDevice) -> u8 {
    let (mut slot, _rx) = device.get_oneshot_slot().unwrap();
    slot.sent = true;
    slot.id
}

#[tokio::test]
async fn late_reply_to_timed_out_request_never_reaches_a_newer_one() {
    let Harness {
        device,
        incoming_tx,
        ..
    } = harness();
    let timed_out = time_out_request(&device);

    // The id is withheld, so the next request gets a different one...
    let (newer, mut newer_rx) = device.get_oneshot_slot().unwrap();
    assert_ne!(newer.id, timed_out);

    // ...and the old request's reply arriving now is dropped, not delivered.
    incoming_tx
        .send(Packet {
            id: timed_out,
            data: PacketData::ReadVersion(),
        })
        .await
        .unwrap();
    // Round-trip a second packet so the dispatcher has certainly handled the first.
    let (probe, probe_rx) = device.get_oneshot_slot().unwrap();
    incoming_tx
        .send(Packet {
            id: probe.id,
            data: PacketData::ReadVersion(),
        })
        .await
        .unwrap();
    probe_rx.await.expect("dispatcher delivers live replies");
    assert!(
        newer_rx.try_recv().is_err(),
        "late reply leaked into a newer request"
    );

    // Consuming the late reply released the id for reuse.
    drop(newer);
    let (reused, _rx) = device.get_oneshot_slot().unwrap();
    assert_eq!(reused.id, timed_out);
}

#[tokio::test(start_paused = true)]
async fn quarantined_id_is_reused_after_the_quarantine() {
    let Harness {
        device,
        incoming_tx: _open,
        ..
    } = harness();
    let timed_out = time_out_request(&device);

    let (during, _rx) = device.get_oneshot_slot().unwrap();
    assert_ne!(during.id, timed_out);
    drop(during);

    tokio::time::advance(ABANDONED_ID_QUARANTINE).await;

    let (after, _rx) = device.get_oneshot_slot().unwrap();
    assert_eq!(after.id, timed_out, "lowest free id is the expired one");
}

#[tokio::test]
async fn write_vendor_refuses_out_of_range_input_instead_of_panicking() {
    let Harness {
        device,
        mut sent,
        incoming_tx: _open,
    } = harness();

    // The range markers themselves, and anything below the range.
    for tag in [0x00, 0x80, 0xff] {
        assert!(device.write_vendor(tag, &[1]).await.is_err(), "tag {tag}");
    }
    // One byte past the 98-byte payload.
    assert!(device.write_vendor(0x90, &[0; 99]).await.is_err());
    assert!(sent.try_recv().is_err(), "nothing was sent");

    device.write_vendor(0x90, &[7; 98]).await.unwrap();
    let packet = sent.try_recv().expect("a valid vendor packet is sent");
    assert!(matches!(packet.data, PacketData::Vendor(0x90, data) if data.len == 98));
}

#[tokio::test]
async fn failed_stream_subscription_does_not_block_the_next_attempt() {
    let Harness {
        device,
        sent,
        incoming_tx: _open,
    } = harness();
    // With the transport's receiving side gone the Enable packet cannot be sent.
    drop(sent);

    for _ in 0..2 {
        let err = device
            .stream(crate::packets::vm::PacketType::ObjectReport())
            .await
            .err()
            .expect("subscription fails without a transport");
        assert!(!err.to_string().contains("more than one"), "{err}");
    }
}

#[tokio::test]
async fn no_reply_id_does_not_kill_the_dispatcher() {
    let Harness {
        device,
        incoming_tx,
        ..
    } = harness();
    incoming_tx
        .send(Packet {
            id: 255,
            data: PacketData::Ack(),
        })
        .await
        .unwrap();
    let (probe, reply) = device.get_oneshot_slot().unwrap();
    incoming_tx
        .send(Packet {
            id: probe.id,
            data: PacketData::Ack(),
        })
        .await
        .unwrap();
    assert!(matches!(reply.await.unwrap(), PacketData::Ack()));
}

#[tokio::test]
async fn replacing_stream_queues_disable_before_enable() {
    use crate::packets::vm::{PacketType, StreamUpdateAction};
    let Harness {
        device,
        mut sent,
        incoming_tx: _open,
    } = harness();
    let stream = device.stream(PacketType::BatteryReport()).await.unwrap();
    sent.recv().await.unwrap();
    drop(stream);
    let replacement = device.stream(PacketType::BatteryReport()).await.unwrap();
    assert!(matches!(
        sent.recv().await.unwrap().data,
        PacketData::StreamUpdate(update) if matches!(update.action, StreamUpdateAction::Disable)
    ));
    assert!(matches!(
        sent.recv().await.unwrap().data,
        PacketData::StreamUpdate(update) if matches!(update.action, StreamUpdateAction::Enable)
    ));
    drop(replacement);
}

#[tokio::test]
async fn cancelling_blocked_stream_enable_releases_its_reservation_in_order() {
    use std::task::Poll;

    use crate::packets::vm::{PacketType, StreamUpdateAction};
    let Harness {
        device,
        mut sent,
        incoming_tx: _open,
    } = harness();
    // Fill the transport queue so Enable cannot be enqueued.
    for _ in 0..64 {
        device
            .send(Packet {
                id: 255,
                data: PacketData::Ack(),
            })
            .await
            .unwrap();
    }
    let mut subscription = Box::pin(device.stream(PacketType::ObjectReport()));
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(subscription.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    drop(subscription);
    // While Disable is also blocked, no replacement may overtake it.
    assert!(device.stream(PacketType::ObjectReport()).await.is_err());
    for _ in 0..64 {
        assert!(matches!(sent.recv().await.unwrap().data, PacketData::Ack()));
    }
    let disable = tokio::time::timeout(Duration::from_secs(1), sent.recv())
        .await
        .expect("cancelled subscription must queue Disable")
        .unwrap();
    assert!(matches!(
        disable.data,
        PacketData::StreamUpdate(update) if matches!(update.action, StreamUpdateAction::Disable)
    ));
    let replacement = device.stream(PacketType::ObjectReport()).await.unwrap();
    assert!(matches!(
        sent.recv().await.unwrap().data,
        PacketData::StreamUpdate(update) if matches!(update.action, StreamUpdateAction::Enable)
    ));
    drop(replacement);
}

#[tokio::test]
async fn clear_streams_propagates_transport_failure() {
    let Harness {
        device,
        sent,
        incoming_tx: _open,
    } = harness();
    drop(sent);
    assert!(device.clear_all_streams().await.is_err());
}

#[tokio::test]
async fn clearing_streams_closes_old_handle_without_disabling_replacement() {
    use tokio_stream::StreamExt as _;

    use crate::packets::vm::{ObjectReport, PacketType, StreamUpdateAction};
    let Harness {
        device,
        mut sent,
        incoming_tx,
    } = harness();
    let mut old = Box::pin(device.stream(PacketType::ObjectReport()).await.unwrap());
    sent.recv().await.unwrap();
    let resetting = tokio::spawn({
        let device = device.clone();
        async move { device.clear_all_streams().await }
    });
    let reset = sent.recv().await.unwrap();
    assert!(matches!(
        reset.data,
        PacketData::StreamUpdate(update) if matches!(update.action, StreamUpdateAction::DisableAll)
    ));
    incoming_tx
        .send(Packet {
            id: reset.id,
            data: PacketData::Ack(),
        })
        .await
        .unwrap();
    resetting.await.unwrap().unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), old.next())
            .await
            .expect("global reset must close the old subscription")
            .is_none()
    );

    let mut replacement = Box::pin(device.stream(PacketType::ObjectReport()).await.unwrap());
    let enable = sent.recv().await.unwrap();
    drop(old);
    assert!(sent.try_recv().is_err(), "stale handle queued a Disable");
    incoming_tx
        .send(Packet {
            id: enable.id,
            data: PacketData::ObjectReport(ObjectReport {
                timestamp: 123,
                ..ObjectReport::default()
            }),
        })
        .await
        .unwrap();
    assert!(matches!(
        replacement.next().await,
        Some(PacketData::ObjectReport(report)) if report.timestamp == 123
    ));
}

#[tokio::test]
async fn pending_clear_fails_fast_when_transport_closes() {
    let Harness {
        device,
        mut sent,
        incoming_tx,
    } = harness();
    let resetting = tokio::spawn(async move { device.clear_all_streams().await });
    sent.recv().await.unwrap();
    drop(incoming_tx);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), resetting)
            .await
            .expect("reset must not keep response channels alive after disconnect")
            .unwrap()
            .is_err()
    );
}
