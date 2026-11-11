use core::cmp;
use core::mem::MaybeUninit;

use embedded_io as eio;
use embedded_io_async as eioa;

/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug)]
pub struct UninitWriteBuf<'a> {
    raw: &'a mut [MaybeUninit<u8>],
    pos: usize,
}

impl<'a> UninitWriteBuf<'a> {
    /// Since: Odyssey Desktop 0.7.0.
    pub fn new(raw: &'a mut [MaybeUninit<u8>]) -> Self {
        Self { raw, pos: 0 }
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub fn capacity(&self) -> usize {
        self.raw.len()
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub fn remaining(&self) -> usize {
        self.capacity().saturating_sub(self.pos)
    }

    fn write_inner(&mut self, src: &[u8]) -> usize {
        let n = cmp::min(self.remaining(), src.len());
        if n != 0 {
            for (slot, b) in self.raw[self.pos..self.pos + n].iter_mut().zip(&src[..n]) {
                slot.write(*b);
            }
            self.pos += n;
        }
        n
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub fn as_init(self) -> &'a mut [u8] {
        let ptr = self.raw.as_ptr() as *mut u8;
        // SAFETY: consumed self retains exclusive access; only the first `pos` bytes were initialized.
        unsafe { core::slice::from_raw_parts_mut(ptr, self.pos) }
    }
}

impl<'a> eio::ErrorType for UninitWriteBuf<'a> {
    type Error = core::convert::Infallible;
}

impl<'a> eio::Write for UninitWriteBuf<'a> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        Ok(self.write_inner(buf))
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<'a> eioa::Write for UninitWriteBuf<'a> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        Ok(self.write_inner(buf))
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
