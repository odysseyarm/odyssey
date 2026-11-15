use std::collections::VecDeque;

use super::*;

#[derive(Default)]
struct Bootloader {
    replies: VecDeque<u8>,
    incoming: Vec<u8>,
    kind: u8,
    init: Vec<u8>,
    image: Vec<u8>,
    accepted: bool,
    signed_for: u8,
    executed: usize,
    rejected: usize,
    corrupt_crc: bool,
    corrupt_offset: bool,
    malformed: bool,
    fail_final_execute: bool,
    installed_size: u32,
    unsupported_layout_query: bool,
}

impl Bootloader {
    fn handle(&mut self, packet: &[u8]) {
        let operation = packet[0];
        let mut payload = Vec::new();
        let mut code = 1;
        match operation {
            PING => payload.push(packet[1]),
            SET_PRN => assert_eq!(&packet[1..], &[0, 0]),
            MTU => payload.extend_from_slice(&31_u16.to_le_bytes()),
            FIRMWARE_VERSION if self.unsupported_layout_query => code = 2,
            FIRMWARE_VERSION => {
                let (kind, address, length) = if packet[1] == 0 {
                    (2, 0xe0000_u32, 0x1e000_u32)
                } else {
                    (1, super::super::layout::APP_ORIGIN, self.installed_size)
                };
                payload.push(kind);
                payload.extend_from_slice(&0_u32.to_le_bytes());
                payload.extend_from_slice(&address.to_le_bytes());
                payload.extend_from_slice(&length.to_le_bytes());
            }
            SELECT => {
                self.kind = packet[1];
                let bytes = if self.kind == COMMAND {
                    &self.init
                } else {
                    &self.image
                };
                payload.extend_from_slice(
                    &(if self.kind == COMMAND { 256_u32 } else { 4 }).to_le_bytes(),
                );
                payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                payload.extend_from_slice(&crc32fast::hash(bytes).to_le_bytes());
            }
            CREATE => {
                self.kind = packet[1];
                if self.kind == COMMAND {
                    self.init.clear();
                    self.image.clear();
                    self.accepted = false;
                } else {
                    assert!(self.accepted, "image erase before signature approval");
                }
            }
            WRITE => {
                if self.kind == COMMAND {
                    self.init.extend_from_slice(&packet[1..]);
                } else {
                    assert!(self.accepted, "image write before signature approval");
                    self.image.extend_from_slice(&packet[1..]);
                }
                return;
            }
            CRC => {
                let bytes = if self.kind == COMMAND {
                    &self.init
                } else {
                    &self.image
                };
                payload.extend_from_slice(
                    &(bytes.len() as u32 + u32::from(self.corrupt_offset)).to_le_bytes(),
                );
                payload.extend_from_slice(
                    &(crc32fast::hash(bytes) ^ u32::from(self.corrupt_crc)).to_le_bytes(),
                );
            }
            EXECUTE if self.kind == COMMAND => {
                if self.init[0] == self.signed_for {
                    self.accepted = true;
                } else {
                    self.rejected += 1;
                    code = 5;
                }
            }
            EXECUTE => {
                if self.fail_final_execute {
                    code = 0x0a;
                } else {
                    self.executed += 1;
                }
            }
            _ => panic!("unexpected DFU operation"),
        }
        let response = if self.malformed {
            vec![RESPONSE, operation]
        } else {
            [vec![RESPONSE, operation, code], payload].concat()
        };
        for byte in response {
            match byte {
                END => self.replies.extend([ESC, ESC_END]),
                ESC => self.replies.extend([ESC, ESC_ESC]),
                _ => self.replies.push_back(byte),
            }
        }
        self.replies.push_back(END);
    }
}

impl Read for Bootloader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let count = buf.len().min(self.replies.len());
        for byte in &mut buf[..count] {
            *byte = self.replies.pop_front().unwrap();
        }
        Ok(count)
    }
}

impl Write for Bootloader {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        assert!(buf.len() <= 31);
        self.incoming.extend_from_slice(buf);
        if self.incoming.last() == Some(&END) {
            let mut decoded = Vec::new();
            let mut escaped = false;
            for &byte in &self.incoming[..self.incoming.len() - 1] {
                if escaped {
                    decoded.push(match byte {
                        ESC_END => END,
                        ESC_ESC => ESC,
                        _ => panic!("bad escape"),
                    });
                    escaped = false;
                } else if byte == ESC {
                    escaped = true;
                } else {
                    decoded.push(byte);
                }
            }
            assert!(!escaped);
            self.incoming.clear();
            self.handle(&decoded);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn package(vendor: u8) -> Package {
    Package {
        init: vec![vendor; 70],
        image: vec![END, ESC, 1, 2, 3, END, ESC, 4, 5],
    }
}

#[test]
fn chooses_accepted_signature_before_writing_and_checks_all_objects() {
    for signed_for in [1, 2] {
        let mut device = Bootloader {
            signed_for,
            image: vec![9; 6],
            ..Default::default()
        };
        let packages = [package(1), package(2)];
        let mut progress = Vec::new();
        transfer(&mut device, &packages, |done, total| {
            progress.push((done, total));
        })
        .unwrap();
        assert_eq!(device.image, packages[0].image);
        assert_eq!(device.executed, 3);
        assert_eq!(device.rejected, usize::from(signed_for == 2));
        assert_eq!(progress, [(0, 9), (4, 9), (8, 9), (9, 9)]);
    }
}

#[test]
fn unknown_vendor_never_erases_or_writes_application() {
    let mut device = Bootloader {
        signed_for: 3,
        ..Default::default()
    };
    assert!(matches!(
        transfer(&mut device, &[package(1), package(2)], |_, _| {}),
        Err(Error::NoMatchingSignature)
    ));
    assert_eq!(device.image, Vec::<u8>::new());
    assert_eq!(device.executed, 0);
}

#[test]
fn corrupted_offset_or_checksum_never_executes_unverified_bytes() {
    for corrupt_offset in [false, true] {
        let mut device = Bootloader {
            signed_for: 1,
            corrupt_offset,
            corrupt_crc: !corrupt_offset,
            ..Default::default()
        };
        assert!(matches!(
            transfer(&mut device, &[package(1)], |_, _| {}),
            Err(Error::Checksum)
        ));
        assert!(!device.accepted);
        assert_eq!(device.executed, 0);
    }
}

#[test]
fn malformed_reply_and_rejected_execute_cannot_report_completion() {
    let mut device = Bootloader {
        malformed: true,
        ..Default::default()
    };
    assert!(matches!(
        transfer(&mut device, &[package(1)], |_, _| {}),
        Err(Error::Protocol(_))
    ));
    let mut device = Bootloader {
        signed_for: 1,
        fail_final_execute: true,
        ..Default::default()
    };
    let mut progress = Vec::new();
    assert!(matches!(
        transfer(&mut device, &[package(1)], |done, _| progress.push(done)),
        Err(Error::Rejected {
            operation: EXECUTE,
            code: 0x0a,
            ..
        })
    ));
    assert_eq!(progress, [0]);
}

#[test]
fn refuses_staging_over_bonds_before_any_init_or_image_write() {
    let mut device = Bootloader {
        installed_size: 417_592,
        ..Default::default()
    };
    let package = Package {
        init: vec![1],
        image: vec![0; 417_592],
    };
    assert!(matches!(
        transfer(&mut device, &[package], |_, _| {}),
        Err(Error::UnsafeLayout)
    ));
    assert_eq!(device.init, Vec::<u8>::new());
    assert_eq!(device.image, Vec::<u8>::new());
}

#[test]
fn missing_layout_query_stops_before_erase() {
    let mut device = Bootloader {
        unsupported_layout_query: true,
        ..Default::default()
    };
    assert!(transfer(&mut device, &[package(1)], |_, _| {}).is_err());
    assert_eq!(device.init, Vec::<u8>::new());
    assert_eq!(device.image, Vec::<u8>::new());
}

#[test]
fn permits_only_bond_safe_bank_or_single_bank_placement() {
    let maximum = super::super::layout::APPLICATION_SIZE as usize;
    assert!(matches!(
        validate_placement(0xe0000, 300_000, 417_592),
        Ok(())
    ));
    assert!(matches!(
        validate_placement(0xe0000, 417_592, maximum),
        Ok(())
    ));
    validate_placement(0xe0000, 417_592, 417_592)
        .expect_err("a placement that ends at the settings page is refused");
    validate_placement(0xe0000, 4096, maximum)
        .expect_err("a placement beyond the application area is refused");
    assert!(matches!(validate_placement(0xe0000, 0, maximum), Ok(())));
    validate_placement(0xe0000, 300_000, maximum + 1)
        .expect_err("a placement one byte past the maximum is refused");
    validate_placement(u32::MAX, u32::MAX, maximum)
        .expect_err("an overflowing placement is refused");
}
