use std::time::Duration;

use tokio::sync::mpsc;

use super::VmDevice;
use crate::Error;
use crate::device::packet_transport::PacketTransport;
use crate::device::response_slots::{ABANDONED_ID_QUARANTINE, ResponseChannel};
use crate::packets::vm::{Packet, PacketData};

struct Harness {
    device: VmDevice,
    /// Packets the device wrote to the transport.
    sent: mpsc::Receiver<Packet>,
    /// Feeds packets "from the device". Dropping it closes the transport.
    incoming_tx: mpsc::Sender<Packet>,
}

fn harness() -> Harness {
    let (writer, sent) = mpsc::channel::<Packet>(64);
    let (incoming_tx, incoming_rx) = mpsc::channel::<Packet>(8);
    let transport = PacketTransport::from_channels(writer, incoming_rx);
    Harness {
        device: VmDevice::from_transport(transport, None),
        sent,
        incoming_tx,
    }
}

#[cfg(feature = "sim")]
#[tokio::test]
async fn simulated_control_retains_pairing_events_and_closes_pending_requests() {
    use odyssey_protocol::control::device::{
        DeviceMsg, PairingError, PairingOutcome, TransportMode,
    };

    let (outgoing, _packets) = mpsc::channel(1);
    let (_incoming, incoming_rx) = mpsc::channel(1);
    let (control_outgoing, mut requests) = mpsc::channel(1);
    let (replies, control_incoming) = mpsc::channel(1);
    let device = VmDevice::from_simulated_channels(
        outgoing,
        incoming_rx,
        control_outgoing,
        control_incoming,
    );
    let request = tokio::spawn({
        let device = device.clone();
        async move { device.get_transport_mode().await }
    });
    assert!(matches!(
        requests.recv().await,
        Some(DeviceMsg::GetTransportMode())
    ));
    replies
        .send(DeviceMsg::PairingResult(PairingOutcome::Err(
            PairingError::Cancelled,
        )))
        .await
        .unwrap();
    replies
        .send(DeviceMsg::TransportModeStatus(TransportMode::Usb))
        .await
        .unwrap();
    assert_eq!(request.await.unwrap().unwrap(), TransportMode::Usb);
    assert!(matches!(
        device.wait_pairing_event().await,
        Err(Error::Pairing(PairingError::Cancelled))
    ));

    let pending = tokio::spawn(async move { device.get_transport_mode().await });
    assert!(matches!(
        requests.recv().await,
        Some(DeviceMsg::GetTransportMode())
    ));
    drop(replies);
    let result = tokio::time::timeout(Duration::from_millis(100), pending)
        .await
        .expect("closed control link must not wait for the reply timeout")
        .unwrap();
    assert!(matches!(result, Err(Error::Closed)));
}

#[cfg(feature = "sim")]
mod bonds {
    use odyssey_protocol::control::device::{
        ClearBondResult, ClearBondsError, DeviceMsg, PairingError, PairingOutcome,
    };
    use odyssey_protocol::control::{AddBondError, AddBondResult, BondEntry, BondSlot};

    use super::*;

    struct Link {
        device: VmDevice,
        /// Control messages the device sent.
        requests: mpsc::Receiver<DeviceMsg>,
        /// Feeds control messages "from the device".
        replies: mpsc::Sender<DeviceMsg>,
    }

    fn link() -> Link {
        let (outgoing, _packets) = mpsc::channel(1);
        let (_incoming, incoming_rx) = mpsc::channel(1);
        let (control_outgoing, requests) = mpsc::channel(4);
        let (replies, control_incoming) = mpsc::channel(4);
        Link {
            device: VmDevice::from_simulated_channels(
                outgoing,
                incoming_rx,
                control_outgoing,
                control_incoming,
            ),
            requests,
            replies,
        }
    }

    fn slot(last: u8, connected: bool) -> BondSlot {
        BondSlot {
            bd_addr: [1, 2, 3, 4, 5, last],
            connected,
        }
    }

    #[tokio::test]
    async fn list_bonds_sends_request_and_decodes_slots() {
        let mut link = link();
        let slots = [slot(6, true), slot(7, false)];
        let list = odyssey_protocol::List::from(heapless::Vec::from_slice(&slots).unwrap());
        link.replies
            .send(DeviceMsg::ListBondsResponse(list))
            .await
            .unwrap();
        assert_eq!(link.device.list_bonds().await.unwrap(), slots);
        assert!(matches!(
            link.requests.recv().await,
            Some(DeviceMsg::ListBonds())
        ));
    }

    #[tokio::test]
    async fn remove_bond_sends_the_address() {
        let mut link = link();
        link.replies
            .send(DeviceMsg::RemoveBondResponse(ClearBondResult::Ok()))
            .await
            .unwrap();
        link.device.remove_bond([1, 2, 3, 4, 5, 6]).await.unwrap();
        assert!(matches!(
            link.requests.recv().await,
            Some(DeviceMsg::RemoveBond([1, 2, 3, 4, 5, 6]))
        ));
    }

    #[tokio::test]
    async fn remove_bond_maps_not_found_and_failure_apart() {
        let link = link();
        for (error, want_not_found) in [
            (ClearBondsError::NotFound, true),
            (ClearBondsError::Failed, false),
        ] {
            link.replies
                .send(DeviceMsg::RemoveBondResponse(ClearBondResult::Err(error)))
                .await
                .unwrap();
            let result = link.device.remove_bond([0; 6]).await;
            assert_eq!(
                matches!(result, Err(Error::BondNotFound)),
                want_not_found,
                "{error:?}"
            );
            assert!(want_not_found || matches!(result, Err(Error::Refused(_))));
        }
    }

    #[tokio::test]
    async fn full_unit_reports_bonds_full_after_start_pairing() {
        let link = link();
        link.replies
            .send(DeviceMsg::StartPairingResponse())
            .await
            .unwrap();
        link.replies
            .send(DeviceMsg::PairingResult(PairingOutcome::Err(
                PairingError::BondsFull,
            )))
            .await
            .unwrap();
        link.device.start_pairing(1000).await.unwrap();
        assert!(matches!(
            link.device.wait_pairing_event().await,
            Err(Error::Pairing(PairingError::BondsFull))
        ));
    }

    #[tokio::test]
    async fn bonds_full_arriving_before_the_ack_is_retained() {
        let link = link();
        link.replies
            .send(DeviceMsg::PairingResult(PairingOutcome::Err(
                PairingError::BondsFull,
            )))
            .await
            .unwrap();
        link.device.start_pairing(1000).await.unwrap();
        assert!(matches!(
            link.device.wait_pairing_event().await,
            Err(Error::Pairing(PairingError::BondsFull))
        ));
    }

    #[tokio::test]
    async fn add_bond_full_is_rejected_distinctly() {
        let link = link();
        link.replies
            .send(DeviceMsg::AddBondResponse(AddBondResult::Err(
                AddBondError::Full,
            )))
            .await
            .unwrap();
        let entry = BondEntry::default();
        assert!(matches!(
            link.device.add_bond(entry).await,
            Err(Error::BondRejected(AddBondError::Full))
        ));
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
    assert!(matches!(err, Error::Closed), "{err:?}");
}

#[tokio::test(start_paused = true)]
async fn zero_retry_attempts_send_no_packet() {
    let Harness {
        device,
        mut sent,
        incoming_tx: _keep_open,
    } = harness();
    let err = device
        .request_with_retry(PacketData::ReadVersion(), Duration::from_secs(2), 0)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::NoResponse { attempts: 0, .. }),
        "{err:?}"
    );
    assert!(matches!(
        sent.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
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
        matches!(err, Error::NoResponse { attempts: 3, .. }),
        "{err:?}"
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

    // The race: the dispatcher takes the sender to deliver the reply, which frees
    // the id. A newer request reuses it. Only then does the original request time
    // out and drop its slot.
    let state = device.thread_state.upgrade().unwrap();
    state.response_channels.lock()[usize::from(id)] = ResponseChannel::None;
    let (newer, _newer_rx) = device.get_oneshot_slot().unwrap();
    assert_eq!(newer.id, id);

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

    // The id is withheld, so the next request gets a different one.
    let (newer, mut newer_rx) = device.get_oneshot_slot().unwrap();
    assert_ne!(newer.id, timed_out);

    // A reply for the timed-out id is dropped, not delivered.
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
        assert!(
            matches!(
                device.write_vendor(tag, &[1]).await,
                Err(Error::InvalidArgument(_))
            ),
            "tag {tag}"
        );
    }
    // One byte past the 98-byte payload.
    assert!(matches!(
        device.write_vendor(0x90, &[0; 99]).await,
        Err(Error::InvalidArgument(_))
    ));
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
            .stream(crate::packets::vm::PacketType::LegacyObjectReport())
            .await
            .err()
            .expect("subscription fails without a transport");
        assert!(matches!(err, Error::Closed), "{err:?}");
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
    let mut subscription = Box::pin(device.stream(PacketType::LegacyObjectReport()));
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(subscription.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    drop(subscription);
    // While Disable is also blocked, no replacement may overtake it.
    assert!(
        device
            .stream(PacketType::LegacyObjectReport())
            .await
            .is_err()
    );
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
    let replacement = device
        .stream(PacketType::LegacyObjectReport())
        .await
        .unwrap();
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

    use crate::packets::vm::legacy::ObjectReport;
    use crate::packets::vm::{PacketType, StreamUpdateAction};
    let Harness {
        device,
        mut sent,
        incoming_tx,
    } = harness();
    let mut old = Box::pin(
        device
            .stream(PacketType::LegacyObjectReport())
            .await
            .unwrap(),
    );
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

    let mut replacement = Box::pin(
        device
            .stream(PacketType::LegacyObjectReport())
            .await
            .unwrap(),
    );
    let enable = sent.recv().await.unwrap();
    drop(old);
    assert!(sent.try_recv().is_err(), "stale handle queued a Disable");
    incoming_tx
        .send(Packet {
            id: enable.id,
            data: PacketData::LegacyObjectReport(ObjectReport {
                timestamp: 123,
                ..ObjectReport::default()
            }),
        })
        .await
        .unwrap();
    assert!(matches!(
        replacement.next().await,
        Some(PacketData::LegacyObjectReport(report)) if report.timestamp == 123
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

#[tokio::test]
async fn report_with_an_id_from_another_link_reaches_the_stream_of_its_type() {
    use tokio_stream::StreamExt as _;

    use crate::packets::vm::{ImpactReport, PacketType};
    let Harness {
        device,
        mut sent,
        incoming_tx,
    } = harness();
    let mut objects = Box::pin(
        device
            .stream(PacketType::LegacyObjectReport())
            .await
            .unwrap(),
    );
    let objects_id = sent.recv().await.unwrap().id;
    let mut impacts = Box::pin(device.stream(PacketType::ImpactReport()).await.unwrap());
    sent.recv().await.unwrap();

    // The unit stamps reports with whatever id the last Enable carried, which may have
    // come over the other link: here one that this link uses for a different stream,
    // then one it never handed out.
    for (id, timestamp) in [(objects_id, 1), (254, 2)] {
        incoming_tx
            .send(Packet {
                id,
                data: PacketData::ImpactReport(ImpactReport { timestamp }),
            })
            .await
            .unwrap();
        let delivered = tokio::time::timeout(Duration::from_secs(1), impacts.next())
            .await
            .expect("report reaches the impact stream");
        assert!(matches!(
            delivered,
            Some(PacketData::ImpactReport(report)) if report.timestamp == timestamp
        ));
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(50), objects.next())
            .await
            .is_err(),
        "the object stream must not see impact reports"
    );
}
