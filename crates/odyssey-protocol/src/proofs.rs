use crate::marker_control::{self, Mode, WriteError};

#[kani::proof]
#[kani::unwind(5)]
fn marker_writes_follow_frozen_gatt_layout() {
    let bytes: [u8; 3] = kani::any();
    let len: usize = kani::any();
    kani::assume(len <= bytes.len());
    let data = &bytes[..len];
    let mode = match data {
        [0] => Ok(Mode::Auto),
        [1] => Ok(Mode::Off),
        [2] => Ok(Mode::On),
        [_] => Err(WriteError::Value),
        _ => Err(WriteError::Length),
    };
    assert_eq!(Mode::parse(data), mode);
    let demand = match data {
        [0] => Ok(false),
        [1] => Ok(true),
        [_] => Err(WriteError::Value),
        _ => Err(WriteError::Length),
    };
    assert_eq!(marker_control::parse_demand(data), demand);
    let pairing = if len == 2 && bytes[0] == 1 && bytes[1] <= 30 {
        Some(bytes[1])
    } else {
        None
    };
    assert_eq!(marker_control::parse_pairing_state(data), pairing);
}

#[cfg(kani)]
#[kani::proof]
#[kani::unwind(10)]
fn ota_decode_never_accepts_noncanonical_frames_or_out_of_bounds_data() {
    let bytes: [u8; 100] = kani::any();
    let length: usize = kani::any();
    kani::assume(length <= bytes.len());
    let result = crate::ota::Request::decode(&bytes[..length]);
    if let Ok(crate::ota::Request::Data { data, .. }) = result {
        assert!(!data.is_empty());
        assert!(length <= crate::ota::MAX_FRAME);
        assert_eq!(data.len() + 5, length);
    }
    if let Ok(crate::ota::Request::Begin { .. }) = result {
        assert_eq!(length, 10);
    }
    if let Ok(crate::ota::Request::Query | crate::ota::Request::Commit | crate::ota::Request::Abort) = result {
        assert_eq!(length, 1);
    }
}

#[cfg(kani)]
#[kani::proof]
#[kani::unwind(4)]
fn ota_status_keeps_frozen_flags_and_little_endian_fields() {
    let offset: u32 = kani::any();
    let version: [u32; 3] = kani::any();
    let status = crate::ota::Status {
        error: crate::ota::Code::Ok,
        app_slot: 2,
        supported: 3,
        staged: 2,
        receiving: Some(crate::ota::Image::Application),
        offset,
        firmware_version: version,
        netcore_release: 0xabcd,
    };
    let bytes = status.encode();
    assert_eq!(bytes[0], crate::ota::PROTOCOL_VERSION);
    assert_eq!(bytes[1], 0);
    assert_eq!(bytes[2], 2);
    assert_eq!(bytes[3], 0b01_1011);
    assert_eq!(bytes[4], offset as u8);
    assert_eq!(bytes[5], (offset >> 8) as u8);
    assert_eq!(bytes[6], (offset >> 16) as u8);
    assert_eq!(bytes[7], (offset >> 24) as u8);
    for index in 0..3 {
        let start = 8 + index * 4;
        assert_eq!(bytes[start], version[index] as u8);
        assert_eq!(bytes[start + 1], (version[index] >> 8) as u8);
        assert_eq!(bytes[start + 2], (version[index] >> 16) as u8);
        assert_eq!(bytes[start + 3], (version[index] >> 24) as u8);
    }
    assert_eq!(bytes[20], 0xcd);
    assert_eq!(bytes[21], 0xab);
    let decoded = crate::ota::Status::decode(&bytes);
    assert!(decoded.is_ok());
    let decoded = decoded.unwrap_or(status);
    assert_eq!(decoded.error, status.error);
    assert_eq!(decoded.app_slot, status.app_slot);
    assert_eq!(decoded.supported, status.supported);
    assert_eq!(decoded.staged, status.staged);
    assert_eq!(decoded.receiving, status.receiving);
    assert_eq!(decoded.offset, offset);
    for index in 0..3 {
        assert_eq!(decoded.firmware_version[index], version[index]);
    }
    assert_eq!(decoded.netcore_release, status.netcore_release);
}

#[cfg(feature = "minicbor")]
#[kani::proof]
#[kani::unwind(32)]
fn device_version_cbor_keeps_frozen_field_order_and_unsigned_encoding() {
    let firmware_semver: [u16; 3] = kani::any();
    let protocol_semver: [u16; 3] = kani::any();
    let version = crate::Version {
        protocol_semver,
        firmware_semver: firmware_semver.map(u32::from),
    };
    let mut actual = [0u8; 21];
    let mut encoder = minicbor::Encoder::new(&mut actual[..]);
    encoder.encode(version).unwrap();
    let used = 21 - encoder.writer().len();
    let mut expected = [0u8; 21];
    expected[0] = 0x82;
    let mut cursor = 1;
    for fields in [protocol_semver, firmware_semver] {
        expected[cursor] = 0x83;
        cursor += 1;
        for value in fields {
            if value < 24 {
                expected[cursor] = value as u8;
                cursor += 1;
            } else if value < 256 {
                expected[cursor] = 0x18;
                expected[cursor + 1] = value as u8;
                cursor += 2;
            } else {
                expected[cursor] = 0x19;
                expected[cursor + 1] = (value / 256) as u8;
                expected[cursor + 2] = (value % 256) as u8;
                cursor += 3;
            }
        }
    }
    assert_eq!(used, cursor);
    assert_eq!(actual, expected);
}

#[cfg(feature = "minicbor")]
#[kani::proof]
#[kani::unwind(8)]
fn device_version_decodes_frozen_unsigned_16_bit_fields() {
    let raw: [u8; 12] = kani::any();
    let bytes = [
        0x82, 0x83, 0x19, raw[0], raw[1], 0x19, raw[2], raw[3], 0x19, raw[4], raw[5], 0x83, 0x19,
        raw[6], raw[7], 0x19, raw[8], raw[9], 0x19, raw[10], raw[11],
    ];
    let decoded: crate::Version = minicbor::decode(&bytes).unwrap();
    for field in 0..3 {
        assert_eq!(
            decoded.protocol_semver[field],
            u16::from(raw[field * 2]) * 256 + u16::from(raw[field * 2 + 1])
        );
        assert_eq!(
            decoded.firmware_semver[field],
            u32::from(raw[6 + field * 2]) * 256 + u32::from(raw[7 + field * 2])
        );
    }
}
