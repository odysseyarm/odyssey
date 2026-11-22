extern crate std;

use std::cell::RefCell;
use std::rc::Rc;
use std::vec::Vec;

use release_signing::{Sha256, dfu_header, message};
use salty::Keypair;

use super::*;

const DEV_SEED: [u8; 32] = [
    0xda, 0x87, 0xce, 0x81, 0xe4, 0xa7, 0x02, 0xa2, 0x7b, 0x21, 0xed, 0xf4, 0xe4, 0x59, 0x4f, 0x6f,
    0xd2, 0x77, 0x18, 0x9e, 0xe3, 0x95, 0x8c, 0x64, 0x8e, 0x50, 0xec, 0x73, 0x35, 0xaa, 0x4b, 0x04,
];

struct NoReset;

impl Reset for NoReset {
    fn sys_reset(&self) {
        panic!("unexpected reset");
    }
}

#[derive(Clone)]
struct Writer {
    bytes: Rc<RefCell<Vec<u8>>>,
    marked: Rc<Cell<bool>>,
    capacity: usize,
}

impl FirmwareWriter for Writer {
    const WRITE_SIZE: usize = 1;
    type Error = ();
    fn write_for_target(&mut self, _: DfuTarget, offset: usize, data: &[u8]) -> Result<(), ()> {
        let end = offset.checked_add(data.len()).ok_or(())?;
        if end > self.capacity {
            return Err(());
        }
        let mut bytes = self.bytes.borrow_mut();
        if offset > bytes.len() {
            return Err(());
        }
        if end > bytes.len() {
            bytes.resize(end, 0xff);
        }
        bytes[offset..end].copy_from_slice(data);
        Ok(())
    }

    fn mark_updated_for_target(&mut self, _: DfuTarget) -> Result<(), ()> {
        self.marked.set(true);
        Ok(())
    }
}

type Signed<'a> = Control<'a, Writer, NoReset, 256>;

fn control(capacity: usize) -> (Signed<'static>, Writer) {
    let writer = Writer {
        bytes: Rc::new(RefCell::new(Vec::new())),
        marked: Rc::new(Cell::new(false)),
        capacity,
    };
    let mut handler = Control::new(
        writer.clone(),
        DfuAttributes::CAN_DOWNLOAD,
        NoReset,
        true,
        [
            Component::Lite1App,
            Component::Lite1NetCore,
            Component::Lite1Bootloader,
        ],
    );
    handler.interface = Some(InterfaceNumber(0));
    (handler, writer)
}

fn signed_image(component: Component, image: &[u8]) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(image);
    let digest = hash.finalize();
    let signed = message(component, image.len() as u32, [1, 2, 3], &digest);
    let signature = Keypair::from(&DEV_SEED).sign(&signed).to_bytes();
    let header = dfu_header(
        component,
        image.len() as u32,
        [1, 2, 3],
        &digest,
        &signature,
    );
    let mut out = header.to_vec();
    out.extend_from_slice(image);
    out
}

fn dnload(handler: &mut Signed<'_>, block: u16, data: &[u8]) -> Option<OutResponse> {
    let [block_lo, block_hi] = block.to_le_bytes();
    let len = u16::try_from(data.len()).expect("test transfer fits USB length");
    let [len_lo, len_hi] = len.to_le_bytes();
    let req = embassy_usb::control::Request::parse(&[
        0x21,
        Request::Dnload as u8,
        block_lo,
        block_hi,
        0,
        0,
        len_lo,
        len_hi,
    ]);
    let response = handler.control_out(req, data);
    if !data.is_empty() && response.is_some() {
        let req = embassy_usb::control::Request::parse(&[
            0xa1,
            Request::GetStatus as u8,
            0,
            0,
            0,
            0,
            6,
            0,
        ]);
        let mut status = [0; 6];
        let _ = handler.control_in(req, &mut status);
    }
    response
}

fn finalize(handler: &mut Signed<'_>, block: u16) -> Option<OutResponse> {
    dnload(handler, block, &[])
}

#[test]
fn signed_image_activates_and_header_does_not_consume_bank_capacity() {
    let image = [0x21; 8];
    let transfer = signed_image(Component::Lite1App, &image);
    let (mut handler, writer) = control(image.len());

    assert!(matches!(
        dnload(&mut handler, 0, &transfer),
        Some(OutResponse::Accepted)
    ));
    assert!(!writer.marked.get());
    assert!(matches!(
        finalize(&mut handler, 1),
        Some(OutResponse::Accepted)
    ));
    assert!(writer.marked.get());
    assert_eq!(writer.bytes.borrow().as_slice(), image);
}

#[test]
fn header_only_block_authenticates_without_writing_empty_payload() {
    let image = [0x42; 8];
    let transfer = signed_image(Component::Lite1App, &image);
    let (mut handler, writer) = control(image.len());

    assert!(matches!(
        dnload(&mut handler, 0, &transfer[..DFU_HEADER_LEN]),
        Some(OutResponse::Accepted)
    ));
    assert!(writer.bytes.borrow().is_empty());
    assert!(!writer.marked.get());
    assert!(matches!(
        dnload(&mut handler, 1, &transfer[DFU_HEADER_LEN..]),
        Some(OutResponse::Accepted)
    ));
    assert!(matches!(
        finalize(&mut handler, 2),
        Some(OutResponse::Accepted)
    ));
    assert!(writer.marked.get());
    assert_eq!(writer.bytes.borrow().as_slice(), image);
}

#[test]
fn unsigned_tampered_wrong_role_and_truncated_images_never_activate() {
    let image = [0x39; 8];
    let (mut unsigned, unsigned_writer) = control(32);
    assert!(matches!(
        dnload(&mut unsigned, 0, &image),
        Some(OutResponse::Rejected)
    ));
    assert!(!unsigned_writer.marked.get());

    let (mut tampered, tampered_writer) = control(32);
    let mut damaged = signed_image(Component::Lite1App, &image);
    damaged[DFU_HEADER_LEN] ^= 1;
    assert!(matches!(
        dnload(&mut tampered, 0, &damaged),
        Some(OutResponse::Accepted)
    ));
    assert!(matches!(
        finalize(&mut tampered, 1),
        Some(OutResponse::Accepted)
    ));
    assert!(!tampered_writer.marked.get());

    let (mut wrong_role, role_writer) = control(32);
    wrong_role.set_alternate_setting(InterfaceNumber(0), 1);
    let wrong = signed_image(Component::Lite1App, &image);
    assert!(matches!(
        dnload(&mut wrong_role, 0, &wrong),
        Some(OutResponse::Rejected)
    ));
    assert!(!role_writer.marked.get());

    let (mut truncated, truncated_writer) = control(32);
    let transfer = signed_image(Component::Lite1App, &image);
    assert!(matches!(
        dnload(&mut truncated, 0, &transfer[..DFU_HEADER_LEN + 4]),
        Some(OutResponse::Accepted)
    ));
    assert!(matches!(
        finalize(&mut truncated, 1),
        Some(OutResponse::Accepted)
    ));
    assert!(!truncated_writer.marked.get());
}

#[test]
fn rejected_order_and_target_switch_recover_after_abort() {
    let image = [0x57; 8];
    let transfer = signed_image(Component::Lite1App, &image);
    let (mut handler, writer) = control(32);

    assert!(matches!(
        dnload(&mut handler, 1, &transfer),
        Some(OutResponse::Rejected)
    ));
    let abort =
        embassy_usb::control::Request::parse(&[0x21, Request::Abort as u8, 0, 0, 0, 0, 0, 0]);
    let _ = handler.control_out(abort, &[]);

    assert!(matches!(
        dnload(&mut handler, 0, &transfer[..DFU_HEADER_LEN + 4]),
        Some(OutResponse::Accepted)
    ));
    assert!(matches!(
        dnload(&mut handler, 0, &transfer),
        Some(OutResponse::Rejected)
    ));
    let abort =
        embassy_usb::control::Request::parse(&[0x21, Request::Abort as u8, 0, 0, 0, 0, 0, 0]);
    let _ = handler.control_out(abort, &[]);
    assert!(matches!(
        dnload(&mut handler, 0, &transfer[..DFU_HEADER_LEN + 4]),
        Some(OutResponse::Accepted)
    ));
    handler.set_alternate_setting(InterfaceNumber(0), 1);
    assert!(matches!(
        dnload(&mut handler, 1, &transfer[DFU_HEADER_LEN + 4..]),
        Some(OutResponse::Rejected)
    ));
    let abort =
        embassy_usb::control::Request::parse(&[0x21, Request::Abort as u8, 0, 0, 0, 0, 0, 0]);
    let _ = handler.control_out(abort, &[]);

    assert!(matches!(
        dnload(&mut handler, 0, &transfer),
        Some(OutResponse::Accepted)
    ));
    assert!(matches!(
        finalize(&mut handler, 1),
        Some(OutResponse::Accepted)
    ));
    assert!(writer.marked.get());
    assert_eq!(writer.bytes.borrow().as_slice(), image);
}
