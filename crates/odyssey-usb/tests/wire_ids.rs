//! Units in the field decode these packets by number, in three encodings: the
//! packed binary ID, the postcard variant index and the CBOR index. The names
//! can change. The numbers cannot.

use odyssey_protocol::legacy::{CombinedMarkersReport, Mode, ObjectReport};
use odyssey_protocol::{MarkersReport, Packet, PacketData};

fn cbor<T: minicbor::Encode<()>>(value: &T) -> Vec<u8> {
    let mut buffer = [0_u8; 1024];
    let mut writer = minicbor::encode::write::Cursor::new(&mut buffer[..]);
    minicbor::encode(value, &mut writer).expect("the buffer holds any packet");
    let written = writer.position();
    buffer[..written].to_vec()
}

struct Case {
    data: PacketData,
    binary_id: u8,
    /// Length of the encoded packet and the bytes that hold its indices.
    postcard: (usize, &'static [u8]),
    cbor: (usize, &'static [u8]),
    type_postcard: &'static [u8],
    type_cbor: &'static [u8],
}

#[test]
fn marker_and_legacy_packets_keep_their_wire_numbers() {
    let markers = MarkersReport {
        points: std::array::from_fn(|i| nalgebra::Point2::new(i as u16, 100 + i as u16)),
    };
    let cases = [
        Case {
            data: PacketData::MarkersReport(markers),
            binary_id: 0x10,
            postcard: (34, &[0x0b, 0x00, 0x64, 0x01, 0x65, 0x02]),
            cbor: (71, &[0x82, 0x82, 0x0b, 0x81, 0x81, 0x90, 0x82, 0x00]),
            type_postcard: &[0x10],
            type_cbor: &[0x82, 0x10, 0x80],
        },
        Case {
            data: PacketData::LegacyObjectReportRequest(),
            binary_id: 0x08,
            postcard: (2, &[0x08, 0x07]),
            cbor: (5, &[0x82, 0x82, 0x08, 0x80, 0x07]),
            type_postcard: &[0x08],
            type_cbor: &[0x82, 0x08, 0x80],
        },
        Case {
            data: PacketData::LegacyObjectReport(ObjectReport::default()),
            binary_id: 0x09,
            postcard: (451, &[0x09, 0x00, 0x00, 0x00, 0x00, 0x00]),
            cbor: (489, &[0x82, 0x82, 0x09, 0x81, 0x83, 0x00, 0x90, 0x8e]),
            type_postcard: &[0x09],
            type_cbor: &[0x82, 0x09, 0x80],
        },
        Case {
            data: PacketData::LegacyCombinedMarkersReport(CombinedMarkersReport::default()),
            binary_id: 0x0a,
            postcard: (66, &[0x0a, 0x00, 0x00, 0x00, 0x00, 0x00]),
            cbor: (104, &[0x82, 0x82, 0x0a, 0x81, 0x82, 0x90, 0x82, 0x00]),
            type_postcard: &[0x0a],
            type_cbor: &[0x82, 0x0a, 0x80],
        },
        Case {
            data: PacketData::LegacyWriteMode(Mode::Object),
            binary_id: 0x11,
            postcard: (3, &[0x11, 0x00, 0x07]),
            cbor: (8, &[0x82, 0x82, 0x11, 0x81, 0x82, 0x00, 0x80, 0x07]),
            type_postcard: &[0x11],
            type_cbor: &[0x82, 0x11, 0x80],
        },
        Case {
            data: PacketData::LegacyWriteMode(Mode::Image),
            binary_id: 0x11,
            postcard: (3, &[0x11, 0x01, 0x07]),
            cbor: (8, &[0x82, 0x82, 0x11, 0x81, 0x82, 0x01, 0x80, 0x07]),
            type_postcard: &[0x11],
            type_cbor: &[0x82, 0x11, 0x80],
        },
    ];
    for case in cases {
        let packet = Packet {
            data: case.data,
            id: 7,
        };
        let ty = packet.ty();
        assert_eq!(u8::from(ty), case.binary_id, "{ty:?}");
        for (name, encoded, (length, start)) in [
            (
                "postcard",
                postcard::to_stdvec(&packet).unwrap(),
                case.postcard,
            ),
            ("CBOR", cbor(&packet), case.cbor),
        ] {
            assert_eq!(encoded.len(), length, "{ty:?} {name}");
            assert_eq!(&encoded[..start.len()], start, "{ty:?} {name}");
        }
        assert_eq!(
            postcard::to_stdvec(&ty).unwrap(),
            case.type_postcard,
            "{ty:?}"
        );
        assert_eq!(cbor(&ty), case.type_cbor, "{ty:?}");
    }
}
