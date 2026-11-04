#![no_std]

use core::convert::Infallible;
use core::future::Future;

use bytemuck::bytes_of_mut;
use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::digital;
use embedded_hal_async::spi::SpiDevice;

pub mod mode;
pub mod spi;
pub mod types;

use mode::{Mode, ModeT};
use spi::Pag7661QnSpi;
use types::IntOStatus;

/// PAG7661QN driver. `M` tracks the sensor mode, either at the type level ([`mode::Idle`],
/// [`mode::Image`], [`mode::Object`]) or at runtime ([`Mode`]). The selected register bank is
/// cached so repeated accesses to the same bank skip the bank-select write.
pub struct Pag7661Qn<I, D, M> {
    bus: I,
    timer: D,
    mode: M,
    bank: u8,
}

/// The PAG7661QN INT_O pin, returned next to the driver by [`Pag7661Qn::init_spi`].
pub struct Pag7661QnInterrupt<O> {
    pub int_o: O,
}

impl<S: SpiDevice, D: DelayNs, M: ModeT> Pag7661Qn<Pag7661QnSpi<S>, D, M> {
    /// Checks the part ID, starts the sensor CPU in `initial_mode` and returns the driver with its
    /// INT_O pin.
    ///
    /// `spi` must be configured for SPI mode 3, MSB first, at most 20 MHz.
    ///
    /// # Errors
    ///
    /// [`InitError::FailedPartIdCheck`] if the part ID is not 0x7660 after one retry,
    /// [`InitError::FailedBootLoader`] if power-on ready is not set 250 ms after the CPU is
    /// enabled.
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
    /// Returns the register interface for direct access.
    ///
    /// Writes through it bypass the cached mode and bank, so the driver's state can disagree with
    /// the device afterwards.
    pub fn bus_unchecked(&mut self) -> &mut I {
        &mut self.bus
    }

    /// Overwrites the cached bank without writing the bank select register (0x7F).
    pub fn set_bank_unchecked(&mut self, bank: u8) {
        self.bank = bank
    }

    /// Changes the mode type without writing the mode register (0x10).
    pub fn set_mode_unchecked<N: ModeT>(self, mode: N) -> Pag7661Qn<I, D, N> {
        Pag7661Qn {
            bus: self.bus,
            timer: self.timer,
            mode,
            bank: self.bank,
        }
    }

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

    async fn read_byte<EO, EM>(&mut self, address: u8) -> Result<u8, Error<I::Error, EO, EM>> {
        self.bus
            .read_byte(address)
            .await
            .map_err(Error::InterfaceError)
    }

    /// Skips the bank select write when the cached bank already matches.
    async fn _set_bank<EO, EM>(&mut self, bank: u8) -> Result<(), Error<I::Error, EO, EM>> {
        if self.bank == bank {
            return Ok(());
        }
        self.write(0x7F, &[bank]).await?;
        self.bank = bank;
        Ok(())
    }

    /// Selects the register bank (0x7F); a no-op when the cached bank already matches.
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
                // Go through idle when changing between image and object mode.
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

    /// Reads the frame rate register (0x13).
    pub async fn sensor_fps(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let mut value = [0];
        self.read(0x13, &mut value).await?;
        Ok(value[0])
    }

    /// Returns `(led_always_on, exposure_us)` from register 0x66: bit 7 is the LED-always-on
    /// flag, bits 6:0 the exposure time in 100 µs steps.
    pub async fn sensor_exposure_us(
        &mut self,
    ) -> Result<(bool, u16), Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let value = self.read_byte(0x66).await?;
        let led_always_on = value & (1 << 7) != 0;
        let exposure_us = u16::from(value & !(1 << 7)) * 100;
        Ok((led_always_on, exposure_us))
    }

    /// Reads the sensor gain register (0x67).
    pub async fn sensor_gain(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        self.read_byte(0x67).await
    }

    /// Reads the object area lower bound (0x68-0x69, little-endian).
    pub async fn area_lower_bound(
        &mut self,
    ) -> Result<u16, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let mut value = 0u16;
        self.read(0x68, bytes_of_mut(&mut value)).await?;
        Ok(value)
    }

    /// Reads the object area upper bound (0x6A-0x6B, little-endian).
    pub async fn area_upper_bound(
        &mut self,
    ) -> Result<u16, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let mut value = 0u16;
        self.read(0x6A, bytes_of_mut(&mut value)).await?;
        Ok(value)
    }

    /// Reads the object light threshold (0x6C).
    pub async fn light_threshold(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        self.read_byte(0x6C).await
    }
}

impl<I: Interface, D: DelayNs, M: mode::IsIdle> Pag7661Qn<I, D, M> {
    /// Writes the frame rate register (0x13). The sensor must be idle.
    pub async fn set_sensor_fps(
        &mut self,
        value: u8,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(0x13, &[value]).await?;
        Ok(())
    }

    /// Writes exposure time and LED mode to register 0x66. The sensor must be idle.
    ///
    /// # Panics
    ///
    /// If `exposure_us` is not a multiple of 100 or exceeds 12700 µs.
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

    /// Writes the sensor gain register (0x67). The sensor must be idle.
    pub async fn set_sensor_gain(
        &mut self,
        x: u8,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(0x67, &[x]).await?;
        Ok(())
    }

    /// Writes the object area lower bound (0x68-0x69, little-endian). The sensor must be idle.
    pub async fn set_area_lower_bound(
        &mut self,
        x: u16,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(0x68, &x.to_le_bytes()).await?;
        Ok(())
    }

    /// Writes the object area upper bound (0x6A-0x6B, little-endian). The sensor must be idle.
    pub async fn set_area_upper_bound(
        &mut self,
        x: u16,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(0x6A, &x.to_le_bytes()).await?;
        Ok(())
    }

    /// Writes the object light threshold (0x6C). The sensor must be idle.
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
    /// Waits on INT_O until a frame is ready, then reads the 320×240 image into `image`.
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
                // Spin until the lock reads back as set.
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
    /// Waits on INT_O until a frame of object data is ready, fills `obj` and returns the number
    /// of objects detected.
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
        let mut total_count = 0;
        loop {
            let status = self.int_o_status().await?;
            if status.frame_ready() {
                // Read object count
                let count = self.read_byte(0x25).await?;

                // At most 8 objects (8 bytes each) are read per report; above 8, the rest
                // arrive in a second report.
                if count > 8 {
                    total_count = count;
                    self.read(0x26, &mut obj_bytes[..8 * 8]).await?;
                    flag = true;
                } else if count > 0 {
                    total_count = count;
                    self.read(0x26, &mut obj_bytes[..count as usize * 8])
                        .await?;
                    done = true;
                } else if flag {
                    // Read 2nd frame
                    self.read(0x26, &mut obj_bytes[8 * 8..total_count as usize * 8])
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
        Ok(Some(total_count))
    }
}

impl<I: Interface, D: DelayNs> Pag7661Qn<I, D, Mode> {
    pub async fn switch_mode_mut(
        &mut self,
        new_mode: Mode,
    ) -> Result<(), Error<I::Error, Infallible, Infallible>> {
        if new_mode.mode() != self.mode.mode() {
            defmt::debug!("Switching mode to {}", new_mode.mode());
            self._set_bank(0x00).await?;
            if self.mode.mode() != Mode::Idle && new_mode.mode() != Mode::Idle {
                // Go through idle when changing between image and object mode.
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
        self.mode = new_mode;
        Ok(())
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

    /// Reads INTO_Status (0x04) and logs its error bit. The caller must have selected bank 0x00.
    async fn int_o_status(&mut self) -> Result<IntOStatus, Self::Error> {
        let value = self.read_byte(0x04).await?;
        let status = IntOStatus::new_with_raw_value(value);
        if status.error() {
            defmt::error!("INTO_Error");
        }
        Ok(status)
    }

    /// Writes INTO_Status (0x04). The caller must have selected bank 0x00.
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
