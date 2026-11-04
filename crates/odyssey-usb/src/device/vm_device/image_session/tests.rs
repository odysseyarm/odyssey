use odyssey_protocol::Port;
use odyssey_protocol::image::{
    IMAGE_CHUNK_CAPACITY, ImageChunk, ImageObject, ImageStatus, ObjectFeature,
};
use tokio_stream::StreamExt as _;

use super::*;
use crate::device::{PacketTransport, VmDevice};

fn link() -> (VmDevice, mpsc::Receiver<Packet>, mpsc::Sender<Packet>) {
    let (outgoing, requests) = mpsc::channel(8);
    let (replies, incoming) = mpsc::channel(8);
    (
        VmDevice::from_transport(PacketTransport::from_channels(outgoing, incoming), None),
        requests,
        replies,
    )
}
fn config() -> ImageStreamConfig {
    ImageStreamConfig {
        camera: Port::Nf,
        fps: 5,
    }
}
async fn status(replies: &mpsc::Sender<Packet>, id: u8, generation: u32, status: ImageStatusCode) {
    replies
        .send(Packet {
            id,
            data: PacketData::ImageStatus(ImageStatus { generation, status }),
        })
        .await
        .unwrap();
}
async fn start(requests: &mut mpsc::Receiver<Packet>, replies: &mpsc::Sender<Packet>) -> (u8, u32) {
    let request = requests.recv().await.unwrap();
    let PacketData::ImageCommand(ImageCommand::Start { generation, .. }) = request.data else {
        panic!("expected Start")
    };
    status(replies, request.id, generation, ImageStatusCode::Started).await;
    (request.id, generation)
}

#[tokio::test]
async fn drop_then_immediate_restart_waits_for_stop_and_excludes_stale_pixels() {
    let (device, mut requests, replies) = link();
    let opening = tokio::spawn({
        let device = device.clone();
        async move { device.stream_images(config()).await }
    });
    let (old_id, old_generation) = start(&mut requests, &replies).await;
    let stream = opening.await.unwrap().unwrap();
    drop(stream);
    let replacement = tokio::spawn({
        let device = device.clone();
        async move { device.stream_images(config()).await }
    });
    let stopping = requests.recv().await.unwrap();
    assert!(
        matches!(stopping.data, PacketData::ImageCommand(ImageCommand::Stop { generation }) if generation == old_generation)
    );
    assert!(
        !replacement.is_finished(),
        "replacement must await the old Stop acknowledgment"
    );
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    status(
        &replies,
        stopping.id,
        old_generation,
        ImageStatusCode::Stopped,
    )
    .await;
    let make = |generation, frame_id, offset, len, byte| ImageChunk {
        generation,
        frame_id,
        timestamp_us: 123,
        objects_timestamp_us: 111,
        camera: Port::Nf,
        width: 98,
        height: 98,
        object_count: 0,
        offset,
        len,
        data: [byte; IMAGE_CHUNK_CAPACITY],
    };
    let request = requests.recv().await.unwrap();
    let PacketData::ImageCommand(ImageCommand::Start {
        generation: new_generation,
        ..
    }) = request.data
    else {
        panic!("expected replacement Start")
    };
    let new_id = request.id;
    // Late data from the old subscription may precede the new acknowledgment
    // when its request id is reused. None of it belongs to the new startup.
    status(&replies, new_id, old_generation, ImageStatusCode::Stopped).await;
    replies
        .send(Packet {
            id: new_id,
            data: PacketData::ImageChunk(make(old_generation, 89, 0, 192, 255)),
        })
        .await
        .unwrap();
    replies
        .send(Packet {
            id: new_id,
            data: PacketData::ImageObject(ImageObject {
                generation: old_generation,
                frame_id: 89,
                index: 0,
                feature: ObjectFeature {
                    id: 0,
                    x: 1.0,
                    y: 1.0,
                    area: 1,
                    mean_brightness: 255,
                    max_brightness: None,
                    bounds: None,
                    radius: None,
                    aspect_ratio: None,
                    is_circle: None,
                },
            }),
        })
        .await
        .unwrap();
    status(&replies, new_id, new_generation, ImageStatusCode::Started).await;
    assert_ne!(old_generation, new_generation);
    let mut stream = replacement.await.unwrap().unwrap();
    // The owned stream keeps the transport alive independently of this handle.
    drop(device);
    tokio::task::yield_now().await;
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    // Reused stream ids are not enough to identify a session.
    replies
        .send(Packet {
            id: old_id,
            data: PacketData::ImageChunk(make(old_generation, 90, 0, 192, 255)),
        })
        .await
        .unwrap();
    for offset in (0..98 * 98).step_by(IMAGE_CHUNK_CAPACITY) {
        replies
            .send(Packet {
                id: new_id,
                data: PacketData::ImageChunk(make(
                    new_generation,
                    1,
                    offset as u32,
                    (98 * 98 - offset).min(IMAGE_CHUNK_CAPACITY) as u16,
                    42,
                )),
            })
            .await
            .unwrap();
    }
    let frame = stream.next().await.unwrap().unwrap();
    assert_eq!(frame.frame_id, 1);
    assert_eq!(frame.pixels, vec![42; 98 * 98]);
    let stopping = tokio::spawn(async move { stream.stop().await });
    let stop = requests.recv().await.unwrap();
    assert!(
        matches!(stop.data, PacketData::ImageCommand(ImageCommand::Stop { generation }) if generation == new_generation)
    );
    status(&replies, stop.id, new_generation, ImageStatusCode::Stopped).await;
    stopping.await.unwrap().unwrap();
}

#[tokio::test]
async fn cancelled_start_still_stops_before_a_successor_can_start() {
    let (device, mut requests, replies) = link();
    let opening = tokio::spawn({
        let device = device.clone();
        async move { device.stream_images(config()).await }
    });
    let request = requests.recv().await.unwrap();
    let PacketData::ImageCommand(ImageCommand::Start { generation, .. }) = request.data else {
        panic!("expected Start")
    };
    opening.abort();
    assert!(matches!(opening.await, Err(error) if error.is_cancelled()));
    let replacement = tokio::spawn({
        let device = device.clone();
        async move { device.stream_images(config()).await }
    });
    let stop = requests.recv().await.unwrap();
    assert!(
        matches!(stop.data, PacketData::ImageCommand(ImageCommand::Stop { generation: old }) if old == generation)
    );
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    status(&replies, stop.id, generation, ImageStatusCode::Stopped).await;
    let (_, next_generation) = start(&mut requests, &replies).await;
    let mut stream = replacement.await.unwrap().unwrap();
    let closing = tokio::spawn(async move { stream.stop().await });
    let stop = requests.recv().await.unwrap();
    status(&replies, stop.id, next_generation, ImageStatusCode::Stopped).await;
    closing.await.unwrap().unwrap();
}

#[tokio::test]
async fn legacy_stop_reports_reboot_and_releases_owner_without_claiming_tracking_recovery() {
    let (device, mut requests, replies) = link();
    let opening = tokio::spawn({
        let device = device.clone();
        async move { device.stream_images(config()).await }
    });
    let (_, generation) = start(&mut requests, &replies).await;
    let mut stream = opening.await.unwrap().unwrap();
    let closing = tokio::spawn(async move { stream.stop().await });
    let stop = requests.recv().await.unwrap();
    status(
        &replies,
        stop.id,
        generation,
        ImageStatusCode::RebootRequired,
    )
    .await;
    assert!(matches!(
        closing.await.unwrap(),
        Err(Error::ImageStatus(ImageStatusCode::RebootRequired))
    ));

    // The host owner is free, but the sensor still requires recovery. A new
    // caller must reach the device, not get Busy from an abandoned host session
    // or a cached Unsupported refusal.
    let successor = tokio::spawn({
        let device = device.clone();
        async move { device.stream_images(config()).await }
    });
    let request = requests.recv().await.unwrap();
    let PacketData::ImageCommand(ImageCommand::Start {
        generation: next, ..
    }) = request.data
    else {
        panic!("expected fresh Start");
    };
    assert_ne!(next, generation);
    status(&replies, request.id, next, ImageStatusCode::RebootRequired).await;
    assert!(matches!(
        successor.await.unwrap(),
        Err(Error::ImageStatus(ImageStatusCode::RebootRequired))
    ));
}

#[tokio::test]
async fn old_firmware_unknown_command_ack_is_cached_as_unsupported() {
    let (device, mut requests, replies) = link();
    let opening = tokio::spawn({
        let device = device.clone();
        async move { device.stream_images(config()).await }
    });
    let request = requests.recv().await.unwrap();
    replies
        .send(Packet {
            id: request.id,
            data: PacketData::Ack(),
        })
        .await
        .unwrap();
    assert!(matches!(
        opening.await.unwrap(),
        Err(Error::ImageUnsupported)
    ));
    assert!(matches!(
        device.stream_images(config()).await,
        Err(Error::ImageUnsupported)
    ));
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn non_usb_link_is_rejected_before_any_wire_request() {
    let (outgoing, mut requests) = mpsc::channel(8);
    let (_replies, incoming) = mpsc::channel(8);
    let mut transport = PacketTransport::from_channels(outgoing, incoming);
    transport.kind = crate::device::packet_transport::LinkKind::Multiplexed;
    let device = VmDevice::from_transport(transport, None);
    assert!(matches!(
        device.stream_images(config()).await,
        Err(Error::ImageUsbRequired)
    ));
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn malformed_frame_is_terminal_even_when_valid_frames_follow_immediately() {
    let (device, mut requests, replies) = link();
    let opening = tokio::spawn({
        let device = device.clone();
        async move { device.stream_images(config()).await }
    });
    let (id, generation) = start(&mut requests, &replies).await;
    let mut stream = opening.await.unwrap().unwrap();
    let base = ImageChunk {
        generation,
        frame_id: 1,
        timestamp_us: 10,
        objects_timestamp_us: 0,
        camera: Port::Nf,
        width: 98,
        height: 98,
        object_count: 0,
        offset: 0,
        len: 193,
        data: [1; IMAGE_CHUNK_CAPACITY],
    };
    replies
        .send(Packet {
            id,
            data: PacketData::ImageChunk(base.clone()),
        })
        .await
        .unwrap();
    for offset in (0..98 * 98).step_by(IMAGE_CHUNK_CAPACITY) {
        let mut chunk = base.clone();
        chunk.frame_id = 2;
        chunk.offset = offset as u32;
        chunk.len = (98 * 98 - offset).min(IMAGE_CHUNK_CAPACITY) as u16;
        replies
            .send(Packet {
                id,
                data: PacketData::ImageChunk(chunk),
            })
            .await
            .unwrap();
    }
    let stop = requests.recv().await.unwrap();
    assert!(
        matches!(stop.data, PacketData::ImageCommand(ImageCommand::Stop { generation: stopping }) if stopping == generation)
    );
    status(&replies, stop.id, generation, ImageStatusCode::Stopped).await;
    assert!(matches!(
        stream.next().await,
        Some(Err(Error::InvalidImage(_)))
    ));
    assert!(stream.next().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn backpressured_start_timeout_does_not_poison_future_sessions_as_unsupported() {
    let (outgoing, mut requests) = mpsc::channel(1);
    outgoing
        .try_send(Packet {
            id: 255,
            data: PacketData::ReadVersion(),
        })
        .unwrap();
    let (replies, incoming) = mpsc::channel(8);
    let device = VmDevice::from_transport(PacketTransport::from_channels(outgoing, incoming), None);
    let opening = tokio::spawn({
        let device = device.clone();
        async move { device.stream_images(config()).await }
    });
    // Neither Start nor its uncertain-start cleanup can reach the full writer.
    // Tokio's paused clock advances through their bounded command deadlines.
    assert!(matches!(opening.await.unwrap(), Err(Error::Timeout(_))));
    let initial = requests.recv().await.unwrap();
    assert!(matches!(initial.data, PacketData::ReadVersion()));
    let retry = tokio::spawn({
        let device = device.clone();
        async move { device.stream_images(config()).await }
    });
    let (_, generation) = start(&mut requests, &replies).await;
    let mut stream = retry.await.unwrap().unwrap();
    let closing = tokio::spawn(async move { stream.stop().await });
    let stop = requests.recv().await.unwrap();
    status(&replies, stop.id, generation, ImageStatusCode::Stopped).await;
    closing.await.unwrap().unwrap();
}
