#![no_std]

use embedded_hal::digital::OutputPin;
use embedded_hal_async::{delay::DelayNs, digital, spi::SpiDevice};

pub mod mode;
pub mod spi;

use mode::{Mode, ModeT};
use spi::Pag7661QnSpi;

/// PAG7661QN driver
pub struct Pag7661Qn<I, D, O, M> {
    bus: I,
    timer: D,
    int_o: O,
    mode: M,
    bank: u8,
}

impl<S: SpiDevice, D: DelayNs, O: digital::Wait, M: ModeT>
    Pag7661Qn<Pag7661QnSpi<S>, D, O, M>
{
    /// Creates a new PAG7661QN driver using SPI.
    ///
    /// Make sure `spi` is configured in mode 3 with MSB first bit order, max 20 MHz.
    pub async fn new_spi(
        spi: S,
        mut timer: D,
        int_o: O,
        initial_mode: M,
    ) -> Result<Self, InitError<S::Error>> {
        defmt::trace!("Pag7661QnSpi::new");
        let mut bus = Pag7661QnSpi::new(spi);

        initialize(&mut bus, &mut timer, initial_mode.mode()).await?;

        Ok(Self {
            bus,
            timer,
            int_o,
            mode: initial_mode,
            bank: 0,
        })
    }
}

async fn initialize<I: Interface, D: DelayNs>(
    bus: &mut I,
    timer: &mut D,
    initial_mode: Mode,
) -> Result<(), InitError<I::Error>> {
    timer.delay_ms(30).await;
    let mut part_id = [0; 2];
    bus.read(0x00, &mut part_id).await?;
    if part_id != [0x60, 0x76] {
        defmt::info!("Part ID check failed, got 0x{=u8:X}{=u8:X}, retrying", part_id[1], part_id[0]);
        // Try again
        timer.delay_ms(30).await;
        bus.read(0x00, &mut part_id).await?;
        if part_id != [0x60, 0x76] {
            defmt::error!("Part ID check failed, got 0x{=u8:X}{=u8:X}", part_id[1], part_id[0]);
            return Err(InitError::FailedPartIdCheck);
        }
    }
    bus.write(0x04, &[initial_mode as u8]).await?;
    bus.write(0x0A, &[1]).await?;
    timer.delay_ms(250).await;
    let mut int_o_status = [0];
    bus.read(0x04, &mut int_o_status).await?;
    if int_o_status[0] & 1 != 1 {
        return Err(InitError::FailedBootLoader);
    }
    bus.write(0x04, &[0]).await?;
    let mut fw_ver = [0; 4];
    bus.read(0x15, &mut fw_ver[..1]).await?;
    bus.read(0x7C, &mut fw_ver[1..]).await?;
    defmt::info!(
        "PAG7661QN firmware version {=u8}.{=u8}.{=u8}.{=u8}",
        fw_ver[0],
        fw_ver[1],
        fw_ver[2],
        fw_ver[3]
    );
    Ok(())
}

impl<I: Interface, D: DelayNs, O: digital::Wait, M: ModeT> Pag7661Qn<I, D, O, M> {
    pub async fn set_bank(&mut self, value: u8) -> Result<(), I::Error> {
        self.bus.write(0x7F, &[value]).await?;
        self.bank = value;
        Ok(())
    }
}

impl<I: Interface, D: DelayNs, O: digital::Wait, M: mode::IsIdle> Pag7661Qn<I, D, O, M>
where
    I::Error: From<M::Error>,
{
    pub async fn set_sensor_fps(&mut self, value: u8) -> Result<(), I::Error> {
        self.mode.is_idle()?;
        if self.bank != 0 {
            self.set_bank(0x00).await?;
        }
        self.bus.write(0x13, &[value]).await?;
        todo!();
    }
}

#[allow(async_fn_in_trait)]
pub trait Interface {
    type Error;
    async fn write(&mut self, address: u8, data: &[u8]) -> Result<(), Self::Error>;
    async fn read(&mut self, address: u8, data: &mut [u8]) -> Result<(), Self::Error>;
}

#[derive(Debug)]
pub enum InitError<E> {
    FailedPartIdCheck,
    FailedBootLoader,
    InterfaceError(E),
}

impl<E> From<E> for InitError<E> {
    fn from(t: E) -> Self {
        InitError::InterfaceError(t)
    }
}
