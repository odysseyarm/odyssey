#![no_std]

use core::{convert::Infallible, future::Future};

use bytemuck::bytes_of_mut;
use embedded_hal_async::{delay::DelayNs, digital, spi::SpiDevice};

pub mod mode;
pub mod spi;
pub mod types;

use mode::{Mode, ModeT};
use spi::Pag7661QnSpi;
use types::IntOStatus;

/// PAG7661QN driver
pub struct Pag7661Qn<I, D, M> {
    bus: I,
    timer: D,
    mode: M,
    bank: u8,
}

/// PAG7661QN interrupt pin
pub struct Pag7661QnInterrupt<O> {
    int_o: O,
}

impl<S: SpiDevice, D: DelayNs, M: ModeT> Pag7661Qn<Pag7661QnSpi<S>, D, M> {
    /// Creates a new PAG7661QN driver using SPI and perform initialization.
    ///
    /// Make sure `spi` is configured in mode 3 with MSB first bit order, max 20 MHz.
    pub async fn init_spi<O: digital::Wait>(
        spi: S,
        mut timer: D,
        int_o: O,
        initial_mode: M,
    ) -> Result<(Self, Pag7661QnInterrupt<O>), InitError<S::Error>> {
        defmt::trace!("Pag7661QnSpi::new");
        let mut bus = Pag7661QnSpi::new(spi);

        initialize(&mut bus, &mut timer, initial_mode.mode()).await?;

        Ok((
            Self {
                bus,
                timer,
                mode: initial_mode,
                bank: 0,
            },
            Pag7661QnInterrupt { int_o },
        ))
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

impl<I: Interface, D: DelayNs, M: ModeT> Pag7661Qn<I, D, M> {
    /// Get the low level driver
    ///
    /// WARNING: using the low level driver can cause the device to become inconsistent with the
    /// high level driver's state such as mode and bank.
    pub fn bus_unchecked(&mut self) -> &mut I {
        &mut self.bus
    }

    /// Set the bank in the driver state without sending any commands to the underlying device.
    pub fn set_bank_unchecked(&mut self, bank: u8) {
        self.bank = bank
    }

    /// Set the mode in the driver state without sending any commands to the underlying device.
    pub fn set_mode_unchecked<N: ModeT>(self, mode: N) -> Pag7661Qn<I, D, N> {
        Pag7661Qn {
            bus: self.bus,
            timer: self.timer,
            mode,
            bank: self.bank,
        }
    }

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

    /// Wrapper to convert errors.
    async fn _set_bank<EO, EM>(&mut self, bank: u8) -> Result<(), Error<I::Error, EO, EM>> {
        if self.bank == bank {
            return Ok(());
        }
        self.write(0x7F, &[bank]).await?;
        self.bank = bank;
        Ok(())
    }

    /// Set the bank
    pub fn set_bank(
        &mut self,
        bank: u8,
    ) -> impl Future<Output = Result<(), Error<I::Error, Infallible, Infallible>>> + use<'_, I, D, M>
    {
        self._set_bank(bank)
    }

    pub fn mode(&self) -> mode::Mode {
        self.mode.mode()
    }

    pub async fn switch_mode<N: ModeT>(
        mut self,
        new_mode: N,
    ) -> Result<Pag7661Qn<I, D, N>, Error<I::Error, Infallible, Infallible>> {
        if new_mode.mode() != self.mode.mode() {
            defmt::debug!("Switching mode to {}", new_mode.mode());
            self._set_bank(0x00).await?;
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
            defmt::debug!("Switched mode to {}", new_mode.mode());
        }
        Ok(Pag7661Qn {
            mode: new_mode,
            bus: self.bus,
            timer: self.timer,
            bank: self.bank,
        })
    }

    pub fn as_dynamic_mode(self) -> Pag7661Qn<I, D, Mode> {
        Pag7661Qn {
            mode: self.mode.mode(),
            bus: self.bus,
            timer: self.timer,
            bank: self.bank,
        }
    }

    async fn int_o_status<EO, EM>(&mut self) -> Result<IntOStatus, Error<I::Error, EO, EM>> {
        self._set_bank(0x00).await?;
        self.bus.int_o_status().await.map_err(Error::InterfaceError)
    }

    async fn set_int_o_status<EO, EM>(
        &mut self,
        value: IntOStatus,
    ) -> Result<(), Error<I::Error, EO, EM>> {
        self._set_bank(0x00).await?;
        self.bus
            .set_int_o_status(value)
            .await
            .map_err(Error::InterfaceError)
    }

    /// Get the sensor FPS.
    pub async fn sensor_fps(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let mut value = [0];
        self.read(0x13, &mut value).await?;
        Ok(value[0])
    }

    /// Get the sensor's exposure time and led mode.
    pub async fn sensor_exposure_us(
        &mut self,
    ) -> Result<(bool, u16), Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let value = self.read_byte(0x66).await?;
        let led_always_on = value & (1 << 7) != 0;
        let exposure_us = u16::from(value & !(1 << 7)) * 100;
        Ok((led_always_on, exposure_us))
    }

    /// Set the sensor gain.
    pub async fn sensor_gain(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        self.read_byte(0x67).await
    }

    /// Set the area lower bound
    pub async fn area_lower_bound(
        &mut self,
    ) -> Result<u16, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let mut value = 0u16;
        self.read(0x68, bytes_of_mut(&mut value)).await?;
        Ok(value)
    }

    /// Set the area upper bound
    pub async fn area_upper_bound(
        &mut self,
    ) -> Result<u16, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let mut value = 0u16;
        self.read(0x6A, bytes_of_mut(&mut value)).await?;
        Ok(value)
    }

    /// Set the light threshold
    pub async fn light_threshold(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        self.read_byte(0x6C).await
    }
}

impl<I: Interface, D: DelayNs, M: mode::IsIdle> Pag7661Qn<I, D, M> {
    /// Set the sensor FPS.
    pub async fn set_sensor_fps(
        &mut self,
        value: u8,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(0x13, &[value]).await?;
        Ok(())
    }

    /// Set the sensor's exposure time and led mode.
    ///
    /// `exposure_us` must be a multiple of 100 and cannot exceed 12700 µs.
    pub async fn set_sensor_exposure_us(
        &mut self,
        led_always_on: bool,
        exposure_us: u16,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        assert!(exposure_us % 100 == 0);
        assert!(exposure_us <= 12700);
        let value = (exposure_us / 100) as u8 | ((led_always_on as u8) << 7);
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(0x66, &[value]).await?;
        Ok(())
    }

    /// Set the sensor gain.
    pub async fn set_sensor_gain(
        &mut self,
        x: u8,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(0x67, &[x]).await?;
        Ok(())
    }

    /// Set the area lower bound
    pub async fn set_area_lower_bound(
        &mut self,
        x: u16,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(0x68, &x.to_le_bytes()).await?;
        Ok(())
    }

    /// Set the area upper bound
    pub async fn set_area_upper_bound(
        &mut self,
        x: u16,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(0x6A, &x.to_le_bytes()).await?;
        Ok(())
    }

    /// Set the light threshold
    pub async fn set_light_threshold(
        &mut self,
        x: u8,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(0x6C, &x.to_le_bytes()).await?;
        Ok(())
    }
}

impl<I: Interface, D: DelayNs, M: mode::IsImage> Pag7661Qn<I, D, M> {
    /// Wait for and get a frame of image data.
    pub async fn wait_for_image<O: digital::Wait>(
        &mut self,
        interrupt: &mut Pag7661QnInterrupt<O>,
        image: &mut [u8; 320 * 240],
    ) -> Result<(), Error<I::Error, O::Error, M::Error>> {
        self.mode.is_image().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;

        loop {
            interrupt.wait().await.map_err(Error::IntOError)?;
            let status = self.int_o_status().await?;
            if status.frame_ready() {
                // Lock frame buffer
                self.write(0x0E, &[1]).await?;
                // Read-back check
                while self.read_byte(0x0E).await? == 0 {}

                // Read image
                self.read(0x0F, image).await?;

                // Unlock frame buffer
                self.write(0x0E, &[0]).await?;
                // Clear frame ready
                self.set_int_o_status(IntOStatus::ZERO).await?;
                break;
            }
        }
        Ok(())
    }
}

impl<I: Interface, D: DelayNs, M: mode::IsObject> Pag7661Qn<I, D, M> {
    /// Wait for and get a frame of object data. Returns the number of objects that were detected.
    pub async fn wait_for_objects<O: digital::Wait>(
        &mut self,
        interrupt: &mut Pag7661QnInterrupt<O>,
        obj: &mut [types::Object; 16],
    ) -> Result<u8, Error<I::Error, O::Error, M::Error>> {
        loop {
            interrupt.wait().await.map_err(Error::IntOError)?;
            let Some(n) = self.try_get_objects(obj).await? else {
                continue;
            };
            return Ok(n);
        }
    }

    /// Get a frame of object data if ready. Returns the number of objects that were detected, or
    /// `Ok(None)` if no object data was ready. Use [`Pag7661QnInterrupt::wait`] to wait until a
    /// frame is ready.
    pub async fn try_get_objects<EO>(
        &mut self,
        obj: &mut [types::Object; 16],
    ) -> Result<Option<u8>, Error<I::Error, EO, M::Error>> {
        self.mode.is_object().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        let obj_bytes = bytemuck::bytes_of_mut(obj);

        let mut done = false;
        let mut flag = false;
        let mut count = 0;
        loop {
            let status = self.int_o_status().await?;
            if status.frame_ready() {
                // Read object count
                count = self.read_byte(0x25).await?;

                // Read object output
                if count > 8 {
                    self.read(0x26, &mut obj_bytes[..8 * 8]).await?;
                    flag = true;
                } else if count > 0 {
                    self.read(0x26, &mut obj_bytes[..count as usize * 8])
                        .await?;
                    done = true;
                } else if flag {
                    // Read 2nd frame
                    self.read(0x26, &mut obj_bytes[8 * 8..count as usize * 8])
                        .await?;
                    done = true;
                } else {
                    done = true;
                }
            } else if !flag {
                self.set_int_o_status(IntOStatus::ZERO).await?;
                return Ok(None);
            }
            self.set_int_o_status(IntOStatus::ZERO).await?;
            if done {
                break;
            }
        }
        Ok(Some(count))
    }
}

impl<O: digital::Wait> Pag7661QnInterrupt<O> {
    pub async fn wait(&mut self) -> Result<(), O::Error> {
        self.int_o.wait_for_high().await
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

    /// Get the value of the INTO_Status register. Make sure the bank is set to 0x00.
    async fn int_o_status(&mut self) -> Result<IntOStatus, Self::Error> {
        let value = self.read_byte(0x04).await?;
        let status = IntOStatus::new_with_raw_value(value);
        if status.error() {
            defmt::error!("INTO_Error");
        }
        Ok(status)
    }

    /// Set the value of the INTO_Status register. Make sure the bank is set to 0x00.
    async fn set_int_o_status(&mut self, value: IntOStatus) -> Result<(), Self::Error> {
        self.write(0x04, &[value.raw_value()]).await?;
        Ok(())
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
