#![cfg(feature = "schema")]

use nalgebra::{Point2, Vector3};
use odyssey_protocol::{
    AccelReport, DeviceName, GeneralConfig, MarkersReport, SetDeviceNameError, SetDeviceNameResult,
    VendorData,
};
use postcard_schema_ng::Schema as _;
use postcard_schema_ng::schema::{Data, DataModelType};

#[test]
fn nalgebra_fields_keep_tuple_encoding() {
    let points =
        core::array::from_fn(|i| Point2::new(100 + u16::try_from(i).expect("16 points"), 300));
    let coordinates = points.map(|p| [p.x, p.y]);
    assert_eq!(
        postcard::to_stdvec(&MarkersReport { points }).expect("markers"),
        postcard::to_stdvec(&coordinates).expect("coordinates")
    );
    let legacy = odyssey_protocol::legacy::CombinedMarkersReport {
        nf_points: points,
        wf_points: points,
    };
    assert_eq!(
        postcard::to_stdvec(&legacy).expect("legacy markers"),
        postcard::to_stdvec(&(coordinates, coordinates)).expect("coordinate arrays")
    );
    let accel = AccelReport {
        timestamp: 300,
        accel: Vector3::new(1.25, -2.5, 3.75),
        gyro: Vector3::new(4.5, 5.25, -6.75),
    };
    assert_eq!(
        postcard::to_stdvec(&accel).expect("acceleration"),
        postcard::to_stdvec(&(300_u32, [1.25_f32, -2.5, 3.75], [4.5_f32, 5.25, -6.75]))
            .expect("scalar arrays")
    );
    let DataModelType::Struct {
        data: Data::Struct(fields),
        ..
    } = MarkersReport::SCHEMA
    else {
        panic!("marker struct schema")
    };
    assert_eq!(fields[0].ty, <[[u16; 2]; 16]>::SCHEMA);
}

#[test]
fn converted_and_transparent_types_follow_their_serde_representation() {
    let config = GeneralConfig::ImpactThreshold(19);
    let wire = odyssey_protocol::wire::GeneralConfig::from(config.clone());
    assert_eq!(
        postcard::to_stdvec(&config).expect("config"),
        postcard::to_stdvec(&wire).expect("wire config")
    );
    assert_eq!(
        GeneralConfig::SCHEMA,
        odyssey_protocol::wire::GeneralConfig::SCHEMA
    );
    let name = DeviceName::default();
    assert_eq!(
        postcard::to_stdvec(&name).expect("device name"),
        postcard::to_stdvec("").expect("string")
    );
    assert_eq!(DeviceName::SCHEMA, <heapless::String<32>>::SCHEMA);
    assert_eq!(
        SetDeviceNameResult::SCHEMA,
        <Result<(), SetDeviceNameError>>::SCHEMA
    );
}

#[test]
fn fixed_bytes_remain_length_prefixed_and_bounded() {
    let vendor = VendorData {
        len: 1,
        data: [7; VendorData::CAPACITY],
    };
    let encoded = postcard::to_stdvec(&vendor).expect("vendor payload");
    assert_eq!(&encoded[..2], &[1, 98]);
    assert_eq!(&encoded[2..], &[7; VendorData::CAPACITY]);
    let DataModelType::Struct {
        data: Data::Struct(fields),
        ..
    } = VendorData::SCHEMA
    else {
        panic!("vendor struct schema")
    };
    assert_eq!(
        fields[1].ty,
        &DataModelType::ByteArray {
            max_len: Some(VendorData::CAPACITY)
        }
    );
}
