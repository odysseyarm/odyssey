use embassy_boot::{AlignedBuffer, BlockingDirectXipState, DirectXipError, Slot};
use embedded_storage::nor_flash::{
    ErrorType, NorFlash, NorFlashError as _, NorFlashErrorKind, ReadNorFlash,
};

use crate::{BootState, BootStateManager, STATE_MAGIC};

struct Journal<'a, F> {
    flash: &'a mut F,
    start: u32,
}

impl<F: NorFlash> ErrorType for Journal<'_, F> {
    type Error = NorFlashErrorKind;
}

impl<F: NorFlash> ReadNorFlash for Journal<'_, F> {
    const READ_SIZE: usize = F::READ_SIZE;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        if (offset as usize)
            .checked_add(bytes.len())
            .is_none_or(|end| end > self.capacity())
        {
            return Err(NorFlashErrorKind::OutOfBounds);
        }
        self.flash
            .read(self.start + offset, bytes)
            .map_err(|e| e.kind())
    }

    fn capacity(&self) -> usize {
        2 * F::ERASE_SIZE
    }
}

impl<F: NorFlash> NorFlash for Journal<'_, F> {
    const WRITE_SIZE: usize = F::WRITE_SIZE;
    const ERASE_SIZE: usize = F::ERASE_SIZE;

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        if (offset as usize)
            .checked_add(bytes.len())
            .is_none_or(|end| end > self.capacity())
        {
            return Err(NorFlashErrorKind::OutOfBounds);
        }
        self.flash
            .write(self.start + offset, bytes)
            .map_err(|e| e.kind())
    }

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        if from > to || to as usize > self.capacity() {
            return Err(NorFlashErrorKind::OutOfBounds);
        }
        self.flash
            .erase(self.start + from, self.start + to)
            .map_err(|e| e.kind())
    }
}

fn slot(index: u8) -> Result<Slot, DirectXipError> {
    match index {
        0 => Ok(Slot::A),
        1 => Ok(Slot::B),
        _ => Err(DirectXipError::BadState),
    }
}

impl BootStateManager {
    /// Returns the writable bank, rejecting pending and unconfirmed trials.
    ///
    /// # Errors
    ///
    /// Returns state/configuration or flash errors; pending trials reject writes.
    pub fn update_slot<F: NorFlash>(&self, flash: &mut F) -> Result<u8, DirectXipError> {
        let mut buffer = AlignedBuffer([0; 32]);
        Ok(self.journal(flash, &mut buffer.0)?.update_slot()?.index() as u8)
    }

    fn journal<'a, F: NorFlash>(
        &self,
        flash: &'a mut F,
        buffer: &'a mut [u8],
    ) -> Result<BlockingDirectXipState<'a, Journal<'a, F>>, DirectXipError> {
        let end = self
            .journal_addr
            .checked_add((2 * F::ERASE_SIZE) as u32)
            .ok_or(DirectXipError::BadState)?;
        if !(self.journal_addr as usize).is_multiple_of(F::ERASE_SIZE)
            || end as usize > flash.capacity()
        {
            return Err(DirectXipError::BadState);
        }
        BlockingDirectXipState::new(
            Journal {
                flash,
                start: self.journal_addr,
            },
            buffer,
        )
        .map_err(|_config| DirectXipError::BadState)
    }

    /// Selects a validated image and records its trial before returning its slot.
    /// A state-write failure leaves boot selection unresolved; enter recovery.
    ///
    /// # Errors
    ///
    /// Returns `NoValidImage` when neither bank validates, or state/flash errors.
    pub fn prepare_boot<F: NorFlash>(
        &self,
        flash: &mut F,
        valid: [bool; 2],
        page_size: u32,
    ) -> Result<u8, DirectXipError> {
        let mut buffer = AlignedBuffer([0; 32]);
        let (chosen, active) = {
            let mut journal = self.journal(flash, &mut buffer.0)?;
            let chosen = journal.prepare_boot(valid)?;
            let active = if journal.is_booted(Slot::A)? {
                Slot::A
            } else {
                Slot::B
            };
            (chosen.index() as u8, active.index() as u8)
        };
        let state = BootState {
            magic: STATE_MAGIC,
            active_slot: active,
            pending_slot: chosen,
            boot_counter: u8::from(chosen != active),
            reserved: 0,
        };
        self.write_boot_state(flash, &state, page_size)?;
        Ok(chosen)
    }

    /// Confirms the slot this application actually executes from.
    ///
    /// # Errors
    ///
    /// Returns `BadState` for an invalid slot or missing trial, or flash errors.
    pub fn mark_running_booted<F: NorFlash>(
        &self,
        flash: &mut F,
        running: u8,
        page_size: u32,
    ) -> Result<(), DirectXipError> {
        let mut buffer = AlignedBuffer([0; 32]);
        self.journal(flash, &mut buffer.0)?
            .mark_booted(slot(running)?)?;
        self.write_boot_state(
            flash,
            &BootState {
                magic: STATE_MAGIC,
                active_slot: running,
                pending_slot: running,
                boot_counter: 0,
                reserved: 0,
            },
            page_size,
        )?;
        Ok(())
    }

    /// Marks a fully written and validated bank for one trial boot.
    ///
    /// # Errors
    ///
    /// Returns `BadState` for the confirmed slot or an active trial, or flash errors.
    pub fn mark_bank_updated<F: NorFlash>(
        &self,
        flash: &mut F,
        bank_slot: u8,
        page_size: u32,
    ) -> Result<(), DirectXipError> {
        let active = self.mark_updated_journal(flash, bank_slot)?;
        self.write_boot_state(
            flash,
            &BootState {
                magic: STATE_MAGIC,
                active_slot: active,
                pending_slot: bank_slot,
                boot_counter: 0,
                reserved: 0,
            },
            page_size,
        )?;
        Ok(())
    }

    pub(super) fn mark_updated_journal<F: NorFlash>(
        &self,
        flash: &mut F,
        bank_slot: u8,
    ) -> Result<u8, DirectXipError> {
        let mut buffer = AlignedBuffer([0; 32]);
        let mut journal = self.journal(flash, &mut buffer.0)?;
        journal.mark_updated(slot(bank_slot)?)?;
        Ok(u8::from(!journal.is_booted(Slot::A)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Flash {
        bytes: [u8; 768],
        interrupt_write: bool,
    }

    impl ErrorType for Flash {
        type Error = NorFlashErrorKind;
    }

    impl ReadNorFlash for Flash {
        const READ_SIZE: usize = 1;

        fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            bytes.copy_from_slice(&self.bytes[offset as usize..offset as usize + bytes.len()]);
            Ok(())
        }

        fn capacity(&self) -> usize {
            self.bytes.len()
        }
    }

    impl NorFlash for Flash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = 256;

        fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            let len = if self.interrupt_write {
                bytes.len() / 2
            } else {
                bytes.len()
            };
            for (dst, src) in self.bytes[offset as usize..].iter_mut().zip(&bytes[..len]) {
                *dst &= src;
            }
            if self.interrupt_write {
                self.interrupt_write = false;
                return Err(NorFlashErrorKind::Other);
            }
            Ok(())
        }

        fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            self.bytes[from as usize..to as usize].fill(0xff);
            Ok(())
        }
    }

    #[test]
    fn trial_rolls_back_unless_running_image_confirms() {
        let mut flash = Flash {
            bytes: [0xff; 768],
            interrupt_write: false,
        };
        let manager = BootStateManager::new(0, 256);
        assert_eq!(manager.prepare_boot(&mut flash, [true, true], 256), Ok(0));
        assert_eq!(manager.mark_bank_updated(&mut flash, 1, 256), Ok(()));
        assert_eq!(
            manager.update_slot(&mut flash),
            Err(DirectXipError::BadState)
        );
        assert_eq!(manager.prepare_boot(&mut flash, [true, true], 256), Ok(1));
        assert_eq!(
            manager.mark_bank_updated(&mut flash, 0, 256),
            Err(DirectXipError::BadState)
        );
        assert_eq!(manager.prepare_boot(&mut flash, [true, true], 256), Ok(0));
        assert_eq!(manager.mark_bank_updated(&mut flash, 1, 256), Ok(()));
        assert_eq!(manager.prepare_boot(&mut flash, [true, true], 256), Ok(1));
        assert_eq!(manager.mark_running_booted(&mut flash, 1, 256), Ok(()));
        assert_eq!(manager.prepare_boot(&mut flash, [true, true], 256), Ok(1));
        assert_eq!(manager.prepare_boot(&mut flash, [true, false], 256), Ok(0));
        assert_eq!(
            manager.prepare_boot(&mut flash, [false, false], 256),
            Err(DirectXipError::NoValidImage)
        );
    }

    #[test]
    fn interrupted_journal_write_preserves_confirmed_image() {
        let mut flash = Flash {
            bytes: [0xff; 768],
            interrupt_write: false,
        };
        let manager = BootStateManager::new(0, 256);
        assert_eq!(manager.prepare_boot(&mut flash, [false, true], 256), Ok(1));
        flash.interrupt_write = true;
        assert!(manager.mark_bank_updated(&mut flash, 0, 256).is_err());
        assert_eq!(manager.prepare_boot(&mut flash, [true, true], 256), Ok(1));
    }
}
