extern crate std;

use embassy_boot::FirmwareUpdaterConfig;
use embedded_storage::nor_flash::{ErrorType, ReadNorFlash};

use self::std::cell::RefCell;
use self::std::rc::Rc;
use super::*;

#[derive(Clone)]
struct Flash<const N: usize>(Rc<RefCell<[u8; N]>>);

impl<const N: usize> Flash<N> {
    fn erased() -> Self {
        Self(Rc::new(RefCell::new([0xff; N])))
    }
}

impl<const N: usize> ErrorType for Flash<N> {
    type Error = NorFlashErrorKind;
}

impl<const N: usize> ReadNorFlash for Flash<N> {
    const READ_SIZE: usize = 1;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        let start = offset as usize;
        let end = start
            .checked_add(bytes.len())
            .ok_or(NorFlashErrorKind::OutOfBounds)?;
        let flash = self.0.borrow();
        bytes.copy_from_slice(
            flash
                .get(start..end)
                .ok_or(NorFlashErrorKind::OutOfBounds)?,
        );
        Ok(())
    }

    fn capacity(&self) -> usize {
        N
    }
}

impl<const N: usize> NorFlash for Flash<N> {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = 4;

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        if !(offset as usize).is_multiple_of(Self::WRITE_SIZE)
            || !bytes.len().is_multiple_of(Self::WRITE_SIZE)
        {
            return Err(NorFlashErrorKind::NotAligned);
        }
        let start = offset as usize;
        let end = start
            .checked_add(bytes.len())
            .ok_or(NorFlashErrorKind::OutOfBounds)?;
        let mut flash = self.0.borrow_mut();
        let dst = flash
            .get_mut(start..end)
            .ok_or(NorFlashErrorKind::OutOfBounds)?;
        for (old, new) in dst.iter_mut().zip(bytes) {
            if *old & new != *new {
                return Err(NorFlashErrorKind::Other);
            }
            *old &= new;
        }
        Ok(())
    }

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        if !(from as usize).is_multiple_of(Self::ERASE_SIZE)
            || !(to as usize).is_multiple_of(Self::ERASE_SIZE)
        {
            return Err(NorFlashErrorKind::NotAligned);
        }
        self.0
            .borrow_mut()
            .get_mut(from as usize..to as usize)
            .ok_or(NorFlashErrorKind::OutOfBounds)?
            .fill(0xff);
        Ok(())
    }
}

struct NoReset;

impl Reset for NoReset {
    fn sys_reset(&self) {
        panic!("transfer must not reset before manifestation status");
    }
}

type Single<'a> = Control<'a, Flash<32>, Flash<256>, NoReset, NullForwarder, NullDualBankWriter, 4>;

fn control<'a>(aligned: &'a mut [u8], capacity: usize) -> (Single<'a>, Flash<32>, Flash<256>) {
    let dfu = Flash::erased();
    let state = Flash::erased();
    let updater = BlockingFirmwareUpdater::new(
        FirmwareUpdaterConfig {
            dfu: dfu.clone(),
            state: state.clone(),
        },
        aligned,
    );
    let mut handler =
        Control::new_single_bank(updater, DfuAttributes::CAN_DOWNLOAD, NoReset, capacity);
    handler.interface = Some(InterfaceNumber(0));
    (handler, dfu, state)
}

fn download(handler: &mut Single<'_>, block: u16, data: &[u8]) {
    let [lo, hi] = block.to_le_bytes();
    let len = u16::try_from(data.len()).expect("test transfer fits USB length");
    let [len_lo, len_hi] = len.to_le_bytes();
    let req = embassy_usb::control::Request::parse(&[
        0x21,
        Request::Dnload as u8,
        lo,
        hi,
        0,
        0,
        len_lo,
        len_hi,
    ]);
    let _ = handler.control_out(req, data);
}

fn status(handler: &mut Single<'_>) {
    let req =
        embassy_usb::control::Request::parse(&[0xa1, Request::GetStatus as u8, 0, 0, 0, 0, 6, 0]);
    let mut reply = [0; 6];
    let _ = handler.control_in(req, &mut reply);
}

#[test]
fn single_bank_pads_last_word_and_commits_only_after_final_block() {
    let mut aligned = AlignedBuffer([0; 4]);
    let (mut handler, dfu, state) = control(aligned.as_mut(), 8);
    download(&mut handler, 0, &[1, 2, 3, 4]);
    status(&mut handler);
    download(&mut handler, 1, &[5]);
    assert_eq!(&dfu.0.borrow()[..8], &[1, 2, 3, 4, 5, 0xff, 0xff, 0xff]);
    assert_eq!(&state.0.borrow()[..4], &[0xff; 4]);
    status(&mut handler);
    download(&mut handler, 2, &[]);
    assert!(matches!(
        handler._updater.get_state(),
        Ok(embassy_boot::State::Swap)
    ));
    assert!(matches!(handler.state, State::ManifestSync));
    assert_eq!(&dfu.0.borrow()[8..], &[0xff; 24]);
}

#[test]
fn single_bank_rejects_empty_out_of_order_and_scratch_overflow() {
    for input in [0, 1, 2] {
        let mut aligned = AlignedBuffer([0; 4]);
        let (mut handler, dfu, state) = control(aligned.as_mut(), 4);
        match input {
            0 => download(&mut handler, 0, &[]),
            1 => download(&mut handler, 1, &[1]),
            _ => {
                download(&mut handler, 0, &[1, 2, 3, 4]);
                status(&mut handler);
                download(&mut handler, 1, &[5]);
            }
        }
        assert!(matches!(handler.state, State::Error), "case {input}");
        assert_eq!(&state.0.borrow()[..4], &[0xff; 4], "case {input}");
        assert_eq!(&dfu.0.borrow()[4..], &[0xff; 28], "case {input}");
    }
}

#[test]
fn single_bank_cannot_select_network_or_bootloader_targets() {
    let mut aligned = AlignedBuffer([0; 4]);
    let (mut handler, _, _) = control(aligned.as_mut(), 4);
    let app_target = handler.target;
    handler.set_alternate_setting(InterfaceNumber(0), 1);
    assert_eq!(handler.target, app_target);
    handler.set_alternate_setting(InterfaceNumber(0), 2);
    assert_eq!(handler.target, app_target);
}
