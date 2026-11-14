use tokio_stream::StreamExt as _;

use super::*;

fn chunks(generation: u32, frame_id: u32) -> Vec<ImageChunk> {
    let pixels: Vec<_> = (0..98 * 98).map(|i| (i % 251) as u8).collect();
    pixels
        .chunks(IMAGE_CHUNK_CAPACITY)
        .enumerate()
        .map(|(index, bytes)| {
            let mut data = [0; IMAGE_CHUNK_CAPACITY];
            data[..bytes.len()].copy_from_slice(bytes);
            ImageChunk {
                generation,
                frame_id,
                timestamp_us: 500,
                objects_timestamp_us: 400,
                camera: Port::Nf,
                width: 98,
                height: 98,
                object_count: 0,
                offset: (index * IMAGE_CHUNK_CAPACITY) as u32,
                len: bytes.len() as u16,
                data,
            }
        })
        .collect()
}

#[test]
fn publishes_exact_complete_pixels_and_rejects_replays() {
    let mut assembler = Reassembler::new(7, Port::Nf);
    let packets = chunks(7, u32::MAX);
    for packet in &packets[..packets.len() - 1] {
        assert_eq!(assembler.chunk(packet).unwrap(), None);
    }
    let frame = assembler.chunk(packets.last().unwrap()).unwrap().unwrap();
    assert_eq!(
        frame.pixels,
        (0u32..98 * 98).map(|i| (i % 251) as u8).collect::<Vec<_>>()
    );
    assert_eq!(frame.timestamp_us, 500);
    assert_eq!(frame.objects_timestamp_us, 400);
    for replay in packets {
        assert_eq!(assembler.chunk(&replay).unwrap(), None);
    }
    // Wrapping frame ids are forward progress, not stale frames.
    let mut next = None;
    for chunk in chunks(7, 0) {
        next = assembler.chunk(&chunk).unwrap().or(next);
    }
    assert_eq!(next.unwrap().frame_id, 0);
}

#[test]
fn malformed_frame_is_discarded_until_a_new_frame_boundary() {
    let mutations: &[fn(&mut ImageChunk)] = &[
        |c| c.width = 321,
        |c| c.height = 0,
        |c| c.len = 193,
        |c| c.len = 0,
        |c| c.offset = u32::MAX,
        |c| c.offset += 1,
        |c| c.timestamp_us += 1,
        |c| c.objects_timestamp_us += 1,
        |c| c.object_count = 17,
        |c| c.camera = Port::Wf,
    ];
    for mutate in mutations {
        let mut assembler = Reassembler::new(7, Port::Nf);
        let packets = chunks(7, 10);
        assembler.chunk(&packets[0]).unwrap();
        let mut malformed = packets[1].clone();
        mutate(&mut malformed);
        assert!(matches!(
            assembler.chunk(&malformed),
            Err(Error::InvalidImage(_))
        ));
        for rest in packets.iter().skip(1) {
            assert_eq!(assembler.chunk(rest).unwrap(), None);
        }
        // The receiver recovers without publishing any pixels from frame 10.
        let mut recovered = None;
        for packet in chunks(7, 11) {
            recovered = assembler.chunk(&packet).unwrap().or(recovered);
        }
        assert_eq!(recovered.unwrap().frame_id, 11);
    }
}

#[test]
fn stale_generations_and_missing_chunks_cannot_complete_partial_frames() {
    let mut assembler = Reassembler::new(8, Port::Nf);
    let fresh = chunks(8, 4);
    assembler.chunk(&fresh[0]).unwrap();
    for stale in chunks(7, 99) {
        assert_eq!(assembler.chunk(&stale).unwrap(), None);
    }
    // Missing offset 192 invalidates this frame even if its lost packet arrives later.
    assert!(matches!(
        assembler.chunk(&fresh[2]),
        Err(Error::InvalidImage(_))
    ));
    for packet in fresh.into_iter().skip(1) {
        assert_eq!(assembler.chunk(&packet).unwrap(), None);
    }
    let mut frame = None;
    for chunk in chunks(8, 5) {
        frame = assembler.chunk(&chunk).unwrap().or(frame);
    }
    assert_eq!(frame.unwrap().frame_id, 5);
}

#[test]
fn metadata_must_be_complete_ordered_and_within_native_image() {
    let feature = ObjectFeature {
        id: 2,
        x: 3.5,
        y: 4.25,
        area: 15,
        mean_brightness: 77,
        max_brightness: None,
        bounds: None,
        radius: None,
        aspect_ratio: None,
        is_circle: Some(true),
    };
    let mut assembler = Reassembler::new(2, Port::Nf);
    assembler
        .object(ImageObject {
            generation: 2,
            frame_id: 1,
            index: 0,
            feature,
        })
        .unwrap();
    let mut frame = None;
    for mut chunk in chunks(2, 1) {
        chunk.object_count = 1;
        frame = assembler.chunk(&chunk).unwrap().or(frame);
    }
    assert_eq!(frame.unwrap().objects, vec![feature]);
    assert!(matches!(
        assembler.object(ImageObject {
            generation: 2,
            frame_id: 2,
            index: 1,
            feature
        }),
        Err(Error::InvalidImage(_))
    ));
    let mut outside = feature;
    outside.x = 98.0;
    assembler
        .object(ImageObject {
            generation: 2,
            frame_id: 3,
            index: 0,
            feature: outside,
        })
        .unwrap();
    let mut initial = chunks(2, 3).remove(0);
    initial.object_count = 1;
    assert!(matches!(
        assembler.chunk(&initial),
        Err(Error::InvalidImage(_))
    ));
}

#[tokio::test]
async fn slow_consumer_receives_latest_complete_frame_not_backlog() {
    let cancel = CancellationToken::new();
    let (_done, stopped) = oneshot::channel();
    let (mut stream, sender) = ImageStream::channel(cancel.clone(), stopped);
    for id in [1, 2, 3] {
        sender.send(Ok(ImageFrame {
            frame_id: id,
            timestamp_us: id,
            camera: Port::Nf,
            width: 98,
            height: 98,
            pixels: vec![id as u8; 98 * 98],
            objects: vec![],
            objects_timestamp_us: 0,
        }));
    }
    let latest = stream.next().await.unwrap().unwrap();
    assert_eq!(latest.frame_id, 3);
    assert_eq!(latest.pixels, vec![3; 98 * 98]);
    drop(stream);
    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn cancelled_stop_remains_awaitable_and_never_exposes_queued_frames() {
    let cancel = CancellationToken::new();
    let (done, stopped) = oneshot::channel();
    let (mut stream, sender) = ImageStream::channel(cancel.clone(), stopped);
    {
        let stopping = stream.stop();
        tokio::pin!(stopping);
        tokio::select! {
            biased;
            result = &mut stopping => panic!("stop completed without acknowledgment: {result:?}"),
            () = tokio::task::yield_now() => {},
        }
    }
    assert!(cancel.is_cancelled());
    sender.send(Ok(ImageFrame {
        frame_id: 9,
        timestamp_us: 9,
        camera: Port::Nf,
        width: 98,
        height: 98,
        pixels: vec![9; 98 * 98],
        objects: vec![],
        objects_timestamp_us: 0,
    }));
    done.send(Ok(())).unwrap();
    stream.stop().await.unwrap();
    assert!(stream.next().await.is_none());
}
