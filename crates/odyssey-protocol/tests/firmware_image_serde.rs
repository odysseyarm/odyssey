#![cfg(feature = "serde")]

use odyssey_protocol::image::{
    IMAGE_CHUNK_CAPACITY, ImageChunk, ImageObject, ImageStatus, ImageStatusCode, ObjectFeature,
};
use odyssey_protocol::{Packet, PacketData, Port};

#[test]
fn image_responses_keep_tags_and_host_device_decode_policy() {
    let responses = [
        PacketData::ImageStatus(ImageStatus {
            generation: 1,
            status: ImageStatusCode::Started,
        }),
        PacketData::ImageChunk(ImageChunk {
            generation: 1,
            frame_id: 1,
            timestamp_us: 1,
            objects_timestamp_us: 0,
            camera: Port::Nf,
            width: 1,
            height: 1,
            object_count: 0,
            offset: 0,
            len: 1,
            data: [7; IMAGE_CHUNK_CAPACITY],
        }),
        PacketData::ImageObject(ImageObject {
            generation: 1,
            frame_id: 1,
            index: 0,
            feature: ObjectFeature {
                id: 0,
                x: 0.0,
                y: 0.0,
                area: 0,
                mean_brightness: 0,
                max_brightness: None,
                bounds: None,
                radius: None,
                aspect_ratio: None,
                is_circle: None,
            },
        }),
    ];
    for (data, tag) in responses.into_iter().zip(25_u8..=27) {
        let packet = Packet { data, id: 0x5a };
        let encoded = postcard::to_stdvec(&packet).expect("serialize image response");
        assert_eq!(encoded[0], tag);
        let decoded = postcard::from_bytes::<Packet>(&encoded);
        if cfg!(all(feature = "device", not(feature = "std"))) {
            assert!(
                matches!(decoded, Err(postcard::Error::SerdeDeCustom)),
                "tag {tag}"
            );
            assert!(matches!(
                postcard::from_bytes::<Packet>(&[tag]),
                Err(postcard::Error::SerdeDeCustom)
            ));
        } else {
            let decoded = decoded.expect("host receives image responses");
            assert_eq!(decoded.id, packet.id);
            assert_eq!(postcard::to_stdvec(&decoded).expect("reencode"), encoded);
        }
    }
    let packet = postcard::from_bytes::<Packet>(&[16, 0x5a]).expect("Ack keeps its wire tag");
    assert!(matches!(packet.data, PacketData::Ack()));
    assert_eq!(packet.id, 0x5a);
}
