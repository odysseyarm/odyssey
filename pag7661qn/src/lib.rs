#![no_std]
use embedded_hal_async::{spi::SpiDevice, delay::DelayNs, digital};
use embedded_hal::digital::OutputPin;

/// PAG7661QN driver
pub struct Pag7661QnSpi<S, D, P, I> {
    spi: S,
    timer: D,
    ssn: P,
    int_o: I,
}

impl<S: SpiDevice, D: DelayNs, P: OutputPin, I: digital::Wait> Pag7661QnSpi<S, D, P, I> {
    /// Creates a new [`Pag7661QnSpi<S, D, P, I>`].
    ///
    /// Make sure `spi` is configured in mode 3 with MSB first bit order.
    pub async fn new(
        spi: S,
        timer: D,
        ssn: P,
        int_o: I,
    ) -> Result<Self, Error<S, P, I>> {
        defmt::trace!("Pag7761QnSpi::new");
        let mut slf = Self {
            spi,
            timer,
            ssn,
            int_o,
        };

        slf.timer.delay_ms(30).await;
        let mut part_id = [0; 2];
        slf.read(0x00, &mut part_id).await?;

        if part_id != [0x60, 0x76] {
            // Try again
            slf.timer.delay_ms(30).await;
            slf.read(0x00, &mut part_id).await?;
            if part_id != [0x60, 0x76] {
                return Err(Error::FailedPartIdCheck);
            }
        }

        // Customize Sensor/LED/Gesture Settings

        // Set Operation Mode to Image Mode
        slf.write(0x04, &[Mode::Image as u8]).await?;

        // Enable the CPU
        slf.write(0x0A, &[1]).await?;

        slf.timer.delay_ms(250).await;
        let mut int_o_status = [0];
        slf.read(0x04, &mut int_o_status).await?;
        if int_o_status[0] | 1 != 1 {
            return Err(Error::FailedBootLoader);
        }
        slf.write(0x04, &[0]).await?;

        let mut fw_ver = [0; 4];
        slf.read(0x15, &mut fw_ver[..1]).await?;
        slf.read(0x7C, &mut fw_ver[1..]).await?;
        defmt::info!("Firmware version {=u8}.{=u8}.{=u8}.{=u8}", fw_ver[0], fw_ver[1], fw_ver[2], fw_ver[3]);

        Ok(slf)
    }

    pub async fn write(&mut self, address: u8, data: &[u8]) -> Result<(), Error<S, P, I>> {
        self.ssn.set_low().map_err(Error::SsnError)?;
        self.spi.write(&[address]).await.map_err(Error::SpiError)?;
        self.spi.write(data).await.map_err(Error::SpiError)?;
        self.ssn.set_high().map_err(Error::SsnError)?;
        Ok(())
    }

    pub async fn read(&mut self, address: u8, data: &mut [u8]) -> Result<(), Error<S, P, I>> {
        if data.len() == 0 {
            return Ok(());
        }
        self.ssn.set_low().map_err(Error::SsnError)?;
        self.spi.write(&[address | 0x80]).await.map_err(Error::SpiError)?;
        self.spi.read(data).await.map_err(Error::SpiError)?;
        self.ssn.set_high().map_err(Error::SsnError)?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum Error<S: SpiDevice, P: OutputPin, I: digital::Wait> {
    FailedPartIdCheck,
    FailedBootLoader,
    SpiError(S::Error),
    SsnError(P::Error),
    IntOError(I::Error),
}

#[derive(Debug)]
#[repr(u8)]
pub enum Mode {
    Idle,
    Image,
    Object,
}
