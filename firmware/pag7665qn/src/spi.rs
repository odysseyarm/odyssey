use embedded_hal::spi::Operation;
use embedded_hal_async::spi::SpiDevice;

use crate::Interface;

/// Register access over SPI: one address byte, then data. Reads set bit 7 of the address.
///
/// Since: Odyssey Desktop 0.6.0.
pub struct Pag7665QnSpi<S> {
    spi: S,
}

impl<S> Pag7665QnSpi<S> {
    /// Since: Odyssey Desktop 0.6.0.
    pub fn new(spi: S) -> Self {
        Self { spi }
    }
}

impl<S: SpiDevice> Interface for Pag7665QnSpi<S> {
    type Error = S::Error;
    async fn write(&mut self, address: u8, data: &[u8]) -> Result<(), S::Error> {
        defmt::trace!(
            "Pag7665QnSpi::write(addr: {=u8:#04X}, data: {=[u8]})",
            address,
            data
        );
        self.spi
            .transaction(&mut [Operation::Write(&[address]), Operation::Write(data)])
            .await?;
        Ok(())
    }

    async fn read(&mut self, address: u8, data: &mut [u8]) -> Result<(), S::Error> {
        defmt::trace!(
            "Pag7665QnSpi::read(addr: {=u8:#04X}, len: {=usize})",
            address,
            data.len()
        );
        if data.is_empty() {
            return Ok(());
        }
        self.spi
            .transaction(&mut [Operation::Write(&[address | 0x80]), Operation::Read(data)])
            .await?;
        defmt::trace!("read result: {=[u8]}", data);
        Ok(())
    }
}
