use crate::marker_control::{self, Mode, WriteError};
use crate::{BootloaderRelease, UsbDeviceRelease, UsbVersionReport};

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

#[kani::proof]
fn usb_application_reports_follow_frozen_bit_layout() {
    let raw: u16 = kani::any();
    let report = UsbDeviceRelease::new_with_raw_value(raw).report();
    if raw == 0 {
        assert_eq!(report, UsbVersionReport::Unset);
    } else if raw >= 0xf000 {
        assert_eq!(report, UsbVersionReport::Unreadable);
    } else {
        let version = [raw / 4096, (raw / 64) % 64, raw % 64];
        assert_eq!(report, UsbVersionReport::Known(version));
        assert_eq!(UsbDeviceRelease::from_version(version).raw_value(), raw);
    }
}

#[kani::proof]
fn usb_bootloader_reports_and_update_order_follow_frozen_layout() {
    let raw: u16 = kani::any();
    let published: u16 = kani::any();
    kani::assume(published > 0 && published <= 4095);
    let report = BootloaderRelease::from_bcd_device(raw);
    let counter = raw % 4096;
    if raw >= 0xf000 && counter != 0 {
        assert_eq!(report, BootloaderRelease::Counter(counter));
        assert_eq!(report.superseded_by(published), counter < published);
    } else {
        assert_eq!(report, BootloaderRelease::Legacy);
        assert!(report.superseded_by(published));
    }
    assert_eq!(BootloaderRelease::bcd_device(published), 61440 + published);
}

#[cfg(feature = "minicbor")]
#[kani::proof]
#[kani::unwind(32)]
fn device_version_cbor_keeps_frozen_field_order_and_unsigned_encoding() {
    let firmware_semver: [u16; 3] = kani::any();
    let protocol_semver: [u16; 3] = kani::any();
    let version = crate::Version {
        protocol_semver,
        firmware_semver,
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
            u16::from(raw[6 + field * 2]) * 256 + u16::from(raw[7 + field * 2])
        );
    }
}
