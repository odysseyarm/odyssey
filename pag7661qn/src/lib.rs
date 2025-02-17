#![no_std]

use core::convert::Infallible;

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

impl<S: SpiDevice, D: DelayNs, O: digital::Wait, M: ModeT> Pag7661Qn<Pag7661QnSpi<S>, D, O, M> {
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
        defmt::info!(
            "Part ID check failed, got 0x{=u8:X}{=u8:X}, retrying",
            part_id[1],
            part_id[0]
        );
        // Try again
        timer.delay_ms(30).await;
        bus.read(0x00, &mut part_id).await?;
        if part_id != [0x60, 0x76] {
            defmt::error!(
                "Part ID check failed, got 0x{=u8:X}{=u8:X}",
                part_id[1],
                part_id[0]
            );
            return Err(InitError::FailedPartIdCheck);
        }
    }
    bus.write(0x10, &[initial_mode as u8]).await?;
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
    /// Wrapper to convert errors.
    async fn read<EO, EM>(
        &mut self,
        address: u8,
        data: &mut [u8],
    ) -> Result<(), Error<I::Error, EO, EM>> {
        self.bus
            .read(address, data)
            .await
            .map_err(Error::InterfaceError)
    }

    /// Wrapper to convert errors.
    async fn write<EO, EM>(
        &mut self,
        address: u8,
        data: &[u8],
    ) -> Result<(), Error<I::Error, EO, EM>> {
        self.bus
            .write(address, data)
            .await
            .map_err(Error::InterfaceError)
    }

    /// Wrapper to convert errors.
    async fn read_byte<EO, EM>(&mut self, address: u8) -> Result<u8, Error<I::Error, EO, EM>> {
        self.bus
            .read_byte(address)
            .await
            .map_err(Error::InterfaceError)
    }

    async fn set_bank<EO, EM>(&mut self, value: u8) -> Result<(), Error<I::Error, EO, EM>> {
        if self.bank == value {
            return Ok(());
        }
        self.write(0x7F, &[value]).await?;
        self.bank = value;
        Ok(())
    }

    pub async fn switch_mode<N: ModeT>(
        mut self,
        new_mode: N,
    ) -> Result<Pag7661Qn<I, D, O, N>, Error<I::Error, Infallible, Infallible>> {
        if new_mode.mode() != self.mode.mode() {
            defmt::debug!("Switching mode to {}", new_mode.mode());
            self.set_bank(0x00).await?;
            if self.mode.mode() != Mode::Idle && new_mode.mode() != Mode::Idle {
                // switch to idle first
                self.write(0x10, &[Mode::Idle as u8]).await?;
                loop {
                    let mut current_mode = [0];
                    self.read(0x14, &mut current_mode).await?;
                    if current_mode[0] == Mode::Idle as u8 {
                        break;
                    }
                }
            }

            self.write(0x10, &[new_mode.mode() as u8]).await?;
            loop {
                let mut current_mode = [0];
                self.read(0x14, &mut current_mode).await?;
                if current_mode[0] == new_mode.mode() as u8 {
                    break;
                }
            }
        }
        defmt::debug!("Switched mode to {}", new_mode.mode());
        Ok(Pag7661Qn {
            mode: new_mode,
            bus: self.bus,
            timer: self.timer,
            int_o: self.int_o,
            bank: self.bank,
        })
    }

    pub async fn get_sensor_fps(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self.set_bank(0x00).await?;
        let mut value = [0];
        self.read(0x13, &mut value).await?;
        Ok(value[0])
    }
}

impl<I: Interface, D: DelayNs, O: digital::Wait, M: mode::IsIdle> Pag7661Qn<I, D, O, M> {
    pub async fn set_sensor_fps(
        &mut self,
        value: u8,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self.set_bank(0x00).await?;
        self.write(0x13, &[value]).await?;
        Ok(())
    }
}

impl<I: Interface, D: DelayNs, O: digital::Wait, M: mode::IsImage> Pag7661Qn<I, D, O, M> {
    pub async fn get_frame(
        &mut self,
        image: &mut [u8; 320 * 240],
    ) -> Result<(), Error<I::Error, O::Error, M::Error>> {
        self.mode.is_image().map_err(Error::ModeError)?;
        self.set_bank(0x00).await?;

        loop {
            self.int_o.wait_for_high().await.map_err(Error::IntOError)?;
            // Check frame ready
            let status = self.read_byte(0x04).await?;
            if status & 0x2 != 0 {
                // Lock frame buffer
                self.write(0x0E, &[1]).await?;
                // Read-back check
                while self.read_byte(0x0E).await? == 0 {}

                // Read image
                self.read(0x0F, image).await?;

                // Unlock frame buffer
                self.write(0x0E, &[0]).await?;
                // Clear frame ready
                self.write(0x04, &[0]).await?;
                break;
            }
        }
        Ok(())
    }
}

#[allow(async_fn_in_trait)]
pub trait Interface {
    type Error;
    async fn write(&mut self, address: u8, data: &[u8]) -> Result<(), Self::Error>;
    async fn read(&mut self, address: u8, data: &mut [u8]) -> Result<(), Self::Error>;
    async fn read_byte(&mut self, address: u8) -> Result<u8, Self::Error> {
        let mut value = [0];
        self.read(address, &mut value).await?;
        Ok(value[0])
    }
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

#[derive(Debug)]
pub enum Error<I, O, M> {
    InterfaceError(I),
    IntOError(O),
    ModeError(M),
}
