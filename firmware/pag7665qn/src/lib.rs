#![no_std]

use core::convert::Infallible;
use core::future::Future;

use bytemuck::bytes_of_mut;
use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::digital;
use embedded_hal_async::spi::SpiDevice;

/// Since: Odyssey Desktop 0.6.0.
pub mod mode;
/// Since: Odyssey Desktop 0.6.0.
pub mod spi;
/// Since: Odyssey Desktop 0.6.0.
pub mod types;

use mode::{Mode, ModeT};
use spi::Pag7665QnSpi;
use types::{CircleParams, IntOStatus, Roi};

/// Maximum time spent waiting for a sensor-controlled mode or report-buffer handshake.
const HANDSHAKE_POLLS: usize = 100;

// PAG7665QN register addresses, bank 0x00 unless noted.
mod regs {
    // Identification
    /// Since: Odyssey Desktop 0.6.0.
    pub const PART_ID_LO: u8 = 0x00;
    /// Since: Odyssey Desktop 0.6.0.
    pub const PART_ID_HI: u8 = 0x01;

    // Status and control
    /// Since: Odyssey Desktop 0.6.0.
    pub const INTO_STATUS: u8 = 0x04;
    /// Since: Odyssey Desktop 0.6.0.
    pub const CPU_RESET_ENL: u8 = 0x0A;
    /// Since: Odyssey Desktop 0.6.0.
    pub const REPORT_BUF_SEL: u8 = 0x0E;
    /// Since: Odyssey Desktop 0.6.0.
    pub const RPT_SRAM_DATA: u8 = 0x0F;

    // Mode control
    /// Since: Odyssey Desktop 0.6.0.
    pub const OP_MODE_HOST: u8 = 0x10;
    /// Since: Odyssey Desktop 0.6.0.
    pub const SENSOR_SIZE_SEL: u8 = 0x12;
    /// Since: Odyssey Desktop 0.6.0.
    pub const SENSOR_FRAME_RATE: u8 = 0x13;
    /// Since: Odyssey Desktop 0.6.0.
    pub const OP_MODE_ACTIVE: u8 = 0x14;

    // Firmware version
    /// Since: Odyssey Desktop 0.6.0.
    pub const FW_MAJOR_VERSION: u8 = 0x15;
    /// Since: Odyssey Desktop 0.6.0.
    pub const FW_MINOR_VERSION: u8 = 0x7C;
    /// Since: Odyssey Desktop 0.6.0.
    pub const FW_CUSTOMER_NUM: u8 = 0x7D;
    /// Since: Odyssey Desktop 0.6.0.
    pub const FW_REVISION_NUM: u8 = 0x7E;

    // Circle detection parameters
    /// Since: Odyssey Desktop 0.6.0.
    pub const CIRCLE_R_LB: u8 = 0x23;
    /// Since: Odyssey Desktop 0.6.0.
    pub const CIRCLE_R_UB: u8 = 0x24;
    /// Since: Odyssey Desktop 0.6.0.
    pub const CIRCLE_K_LB: u8 = 0x77;
    /// Since: Odyssey Desktop 0.6.0.
    pub const CIRCLE_K_UB: u8 = 0x78;

    // Object output
    /// Since: Odyssey Desktop 0.6.0.
    pub const OBJ_COUNT: u8 = 0x25;
    /// Since: Odyssey Desktop 0.6.0.
    pub const OBJ_DATA_START: u8 = 0x26; // 8 bytes per object, 8 objects max per read

    // Manual power control
    /// Since: Odyssey Desktop 0.6.0.
    pub const MANUAL_POWER_CTRL: u8 = 0x2F;
    /// Since: Odyssey Desktop 0.6.0.
    pub const MANUAL_POWER_TRIG: u8 = 0x30;

    // Sensor settings. These addresses differ from the PAG7661QN.
    /// Since: Odyssey Desktop 0.6.0.
    pub const SENSOR_EXPO: u8 = 0x67; // bit[7]=LED_Always_ON, bits[6:0]=time in 100 µs steps
    /// Since: Odyssey Desktop 0.6.0.
    pub const SENSOR_GAIN: u8 = 0x68; // Analog gain
    /// Since: Odyssey Desktop 0.6.0.
    pub const OBJ_LIGHT_THRD: u8 = 0x6D;
    /// Since: Odyssey Desktop 0.6.0.
    pub const OBJ_AREA_LB_LO: u8 = 0x6E; // Area lower bound [7:0]
    /// Since: Odyssey Desktop 0.6.0.
    pub const OBJ_AREA_LB_HI: u8 = 0x6F; // Area lower bound [15:8]
    /// Since: Odyssey Desktop 0.6.0.
    pub const OBJ_AREA_UB_LO: u8 = 0x70; // Area upper bound [7:0]
    /// Since: Odyssey Desktop 0.6.0.
    pub const OBJ_AREA_UB_HI: u8 = 0x71; // Area upper bound [15:8]

    // ROI (Region of Interest)
    /// Since: Odyssey Desktop 0.6.0.
    pub const ROI_CTRL: u8 = 0x72; // bit[5]=X1[8], bit[4]=X0[8], bit[0]=ROI_EN
    /// Since: Odyssey Desktop 0.6.0.
    pub const ROI_X0_LO: u8 = 0x73; // X0[7:0]
    /// Since: Odyssey Desktop 0.6.0.
    pub const ROI_Y0: u8 = 0x74; // Y0[7:0]
    /// Since: Odyssey Desktop 0.6.0.
    pub const ROI_X1_LO: u8 = 0x75; // X1[7:0]
    /// Since: Odyssey Desktop 0.6.0.
    pub const ROI_Y1: u8 = 0x76; // Y1[7:0]

    // Error registers
    /// Since: Odyssey Desktop 0.6.0.
    pub const FW_ERROR_TYPE: u8 = 0x7A;
    /// Since: Odyssey Desktop 0.6.0.
    pub const FW_ERROR_CODE: u8 = 0x7B;

    // Bank select, reachable from every bank.
    /// Since: Odyssey Desktop 0.6.0.
    pub const BANK_NO: u8 = 0x7F;

    // Bank 0x06 registers
    /// Since: Odyssey Desktop 0.6.0.
    pub mod bank6 {
        /// Since: Odyssey Desktop 0.6.0.
        pub const SW_RESET: u8 = 0x1F;
    }
}

/// PAG7665QN driver. `M` tracks the sensor mode, either at the type level ([`mode::Idle`],
/// [`mode::Image`], [`mode::Object`]) or at runtime ([`Mode`]). The selected register bank is
/// cached so repeated accesses to the same bank skip the bank-select write.
///
/// Since: Odyssey Desktop 0.6.0.
pub struct Pag7665Qn<I, D, M> {
    bus: I,
    timer: D,
    mode: M,
    bank: u8,
}

/// The PAG7665QN INT_O pin, returned next to the driver by [`Pag7665Qn::init_spi`].
///
/// Since: Odyssey Desktop 0.6.0.
pub struct Pag7665QnInterrupt<O> {
    /// Since: Odyssey Desktop 0.6.0.
    pub int_o: O,
}

impl<S: SpiDevice, D: DelayNs, M: ModeT> Pag7665Qn<Pag7665QnSpi<S>, D, M> {
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
    ///
    /// Initialization also returns an interface error if an SPI operation fails.
    /// # Since
    /// Odyssey Desktop 0.6.0.
    pub async fn init_spi<O: digital::Wait>(
        spi: S,
        mut timer: D,
        int_o: O,
        initial_mode: M,
    ) -> Result<(Self, Pag7665QnInterrupt<O>), InitError<S::Error>> {
        defmt::trace!("Pag7665QnSpi::new");
        let mut bus = Pag7665QnSpi::new(spi);

        initialize(&mut bus, &mut timer, initial_mode.mode()).await?;

        Ok((
            Self {
                bus,
                timer,
                mode: initial_mode,
                bank: 0,
            },
            Pag7665QnInterrupt { int_o },
        ))
    }
}

async fn initialize<I: Interface, D: DelayNs>(
    bus: &mut I,
    timer: &mut D,
    initial_mode: Mode,
) -> Result<(), InitError<I::Error>> {
    timer.delay_ms(30).await;

    // The PAG7665QN reports part ID 0x7660.
    let mut part_id = [0; 2];
    bus.read(regs::PART_ID_LO, &mut part_id).await?;
    if part_id != [0x60, 0x76] {
        defmt::info!(
            "Part ID check failed, got 0x{=u8:X}{=u8:X}, retrying",
            part_id[1],
            part_id[0]
        );
        timer.delay_ms(30).await;
        bus.read(regs::PART_ID_LO, &mut part_id).await?;
        if part_id != [0x60, 0x76] {
            defmt::error!(
                "Part ID check failed, got 0x{=u8:X}{=u8:X}",
                part_id[1],
                part_id[0]
            );
            return Err(InitError::FailedPartIdCheck);
        }
    }

    // CPU_RESET_ENL=1 starts the sensor CPU in the mode written to OP_MODE_HOST.
    bus.write(regs::OP_MODE_HOST, &[initial_mode as u8]).await?;
    bus.write(regs::CPU_RESET_ENL, &[1]).await?;

    // The datasheet allows up to 300 ms for power-on ready. This waits 250 ms.
    timer.delay_ms(250).await;
    let mut int_o_status = [0];
    bus.read(regs::INTO_STATUS, &mut int_o_status).await?;
    if int_o_status[0] & 1 != 1 {
        return Err(InitError::FailedBootLoader);
    }

    bus.write(regs::INTO_STATUS, &[0]).await?;

    let mut fw_ver = [0; 4];
    bus.read(regs::FW_MAJOR_VERSION, &mut fw_ver[..1]).await?;
    bus.read(regs::FW_MINOR_VERSION, &mut fw_ver[1..]).await?;
    defmt::info!(
        "PAG7665QN firmware version {=u8}.{=u8}.{=u8}.{=u8}",
        fw_ver[0],
        fw_ver[1],
        fw_ver[2],
        fw_ver[3]
    );

    Ok(())
}

impl<I: Interface, D: DelayNs, M: ModeT> Pag7665Qn<I, D, M> {
    /// Returns the register interface for direct access.
    ///
    /// Writes through it bypass the cached mode and bank, so the driver's state can disagree with
    /// the device afterwards.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn bus_unchecked(&mut self) -> &mut I {
        &mut self.bus
    }

    /// Overwrites the cached bank without writing BANK_NO (0x7F).
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn set_bank_unchecked(&mut self, bank: u8) {
        self.bank = bank
    }

    /// Changes the mode type without writing OP_MODE_HOST (0x10).
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub fn set_mode_unchecked<N: ModeT>(self, mode: N) -> Pag7665Qn<I, D, N> {
        Pag7665Qn {
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

    /// Skips the BANK_NO write when the cached bank already matches.
    async fn _set_bank<EO, EM>(&mut self, bank: u8) -> Result<(), Error<I::Error, EO, EM>> {
        if self.bank == bank {
            return Ok(());
        }
        self.write(regs::BANK_NO, &[bank]).await?;
        self.bank = bank;
        Ok(())
    }

    /// Selects the register bank (BANK_NO, 0x7F). A no-op when the cached bank already matches.
    ///
    /// # Errors
    /// Returns an interface error if writing the selected bank fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub fn set_bank(
        &mut self,
        bank: u8,
    ) -> impl Future<Output = Result<(), Error<I::Error, Infallible, Infallible>>> + use<'_, I, D, M>
    {
        self._set_bank(bank)
    }

    /// Since: Odyssey Desktop 0.6.0.
    pub fn mode(&self) -> mode::Mode {
        self.mode.mode()
    }

    async fn write_mode_and_wait<EO, EM>(
        &mut self,
        mode: Mode,
    ) -> Result<(), Error<I::Error, EO, EM>> {
        // Object-ready and image-ready share one flag; do not inherit a previous mode's
        // report as the first frame of the new mode.
        self.set_int_o_status(IntOStatus::ZERO).await?;
        self.write(regs::OP_MODE_HOST, &[mode as u8]).await?;
        for _ in 0..HANDSHAKE_POLLS {
            if self.read_byte(regs::OP_MODE_ACTIVE).await? == mode as u8 {
                return Ok(());
            }
            self.timer.delay_ms(1).await;
        }
        Err(Error::Timeout)
    }

    /// # Errors
    /// Returns an interface error on SPI failure or [`Error::Timeout`] if sensor does not acknowledge the mode.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn switch_mode<N: ModeT>(
        mut self,
        new_mode: N,
    ) -> Result<Pag7665Qn<I, D, N>, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let current_mode = self.read_byte(regs::OP_MODE_ACTIVE).await?;
        if current_mode != new_mode.mode() as u8 {
            if current_mode != Mode::Idle as u8 && new_mode.mode() != Mode::Idle {
                self.write_mode_and_wait(Mode::Idle).await?;
            }
            self.write_mode_and_wait(new_mode.mode()).await?;
        }
        Ok(Pag7665Qn {
            mode: new_mode,
            bus: self.bus,
            timer: self.timer,
            bank: self.bank,
        })
    }

    /// Since: Odyssey Desktop 0.6.0.
    pub fn as_dynamic_mode(self) -> Pag7665Qn<I, D, Mode> {
        Pag7665Qn {
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

    /// Reads SENSOR_FRAME_RATE (0x13).
    ///
    /// # Errors
    /// Returns an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn sensor_fps(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let mut value = [0];
        self.read(regs::SENSOR_FRAME_RATE, &mut value).await?;
        Ok(value[0])
    }

    /// Returns `(led_always_on, exposure_us)` from SENSOR_EXPO (0x67): bit 7 is the
    /// LED-always-on flag, bits 6:0 the exposure time in 100 µs steps.
    ///
    /// # Errors
    /// Returns an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn sensor_exposure_us(
        &mut self,
    ) -> Result<(bool, u16), Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let value = self.read_byte(regs::SENSOR_EXPO).await?;
        let led_always_on = value & (1 << 7) != 0;
        let exposure_us = u16::from(value & !(1 << 7)) * 100;
        Ok((led_always_on, exposure_us))
    }

    /// Reads the analog gain (SENSOR_GAIN, 0x68).
    ///
    /// # Errors
    /// Returns an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn sensor_gain(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        self.read_byte(regs::SENSOR_GAIN).await
    }

    /// Reads the object area lower bound (0x6E-0x6F, little-endian).
    ///
    /// # Errors
    /// Returns an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn area_lower_bound(
        &mut self,
    ) -> Result<u16, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let mut value = 0u16;
        self.read(regs::OBJ_AREA_LB_LO, bytes_of_mut(&mut value))
            .await?;
        Ok(value)
    }

    /// Reads the object area upper bound (0x70-0x71, little-endian).
    ///
    /// # Errors
    /// Returns an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn area_upper_bound(
        &mut self,
    ) -> Result<u16, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let mut value = 0u16;
        self.read(regs::OBJ_AREA_UB_LO, bytes_of_mut(&mut value))
            .await?;
        Ok(value)
    }

    /// Reads OBJ_LIGHT_THRD (0x6D).
    ///
    /// # Errors
    /// Returns an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn light_threshold(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        self.read_byte(regs::OBJ_LIGHT_THRD).await
    }

    /// Reads the circle detection ratio bounds (0x23, 0x24, 0x77, 0x78).
    ///
    /// # Errors
    /// Returns an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn circle_params(
        &mut self,
    ) -> Result<CircleParams, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let r_lb = self.read_byte(regs::CIRCLE_R_LB).await?;
        let r_ub = self.read_byte(regs::CIRCLE_R_UB).await?;
        let k_lb = self.read_byte(regs::CIRCLE_K_LB).await?;
        let k_ub = self.read_byte(regs::CIRCLE_K_UB).await?;
        Ok(CircleParams {
            r_lower_bound: r_lb,
            r_upper_bound: r_ub,
            k_lower_bound: k_lb,
            k_upper_bound: k_ub,
        })
    }

    /// Reads the ROI registers (0x72-0x76) and reassembles the 9-bit X coordinates.
    ///
    /// # Errors
    /// Returns an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn roi(&mut self) -> Result<Roi, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let ctrl = self.read_byte(regs::ROI_CTRL).await?;
        let x0_lo = self.read_byte(regs::ROI_X0_LO).await?;
        let y0 = self.read_byte(regs::ROI_Y0).await?;
        let x1_lo = self.read_byte(regs::ROI_X1_LO).await?;
        let y1 = self.read_byte(regs::ROI_Y1).await?;

        let enabled = ctrl & 1 != 0;
        let x0 = u16::from(x0_lo) | (u16::from((ctrl >> 4) & 1) << 8);
        let x1 = u16::from(x1_lo) | (u16::from((ctrl >> 5) & 1) << 8);

        Ok(Roi {
            enabled,
            x0,
            y0,
            x1,
            y1,
        })
    }

    /// Writes 0xFF to SW_RESET in bank 0x06 and waits 1 ms. All registers are cleared.
    ///
    /// # Errors
    /// Returns an interface error if the reset write fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn software_reset(&mut self) -> Result<(), Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x06).await?;
        self.write(regs::bank6::SW_RESET, &[0xFF]).await?;
        self.timer.delay_ms(1).await;
        self.bank = 0; // The reset leaves the device in bank 0x00.
        Ok(())
    }
}

impl<I: Interface, D: DelayNs, M: mode::IsIdle> Pag7665Qn<I, D, M> {
    /// Writes SENSOR_FRAME_RATE (0x13). The sensor must be idle.
    ///
    /// # Errors
    /// Returns a mode error if runtime mode is not idle, or an interface error if register access fails.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn set_sensor_fps(
        &mut self,
        value: u8,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(regs::SENSOR_FRAME_RATE, &[value]).await?;
        Ok(())
    }

    /// Writes exposure time and LED mode to SENSOR_EXPO (0x67). The sensor must be idle.
    ///
    /// # Panics
    ///
    /// Panics if `exposure_us` is not a multiple of 100 or exceeds 12,700 µs.
    ///
    /// # Errors
    ///
    /// Returns a mode error if runtime mode is not idle, or an interface error if register access fails.
    ///
    /// # Since
    /// Odyssey Desktop 0.6.0.
    ///
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
        self.write(regs::SENSOR_EXPO, &[value]).await?;
        Ok(())
    }

    /// Writes the analog gain (SENSOR_GAIN, 0x68). The sensor must be idle.
    ///
    ///
    /// # Errors
    /// Returns a mode error if runtime mode is not idle, or an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn set_sensor_gain(
        &mut self,
        x: u8,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(regs::SENSOR_GAIN, &[x]).await?;
        Ok(())
    }

    /// Writes the object area lower bound (0x6E-0x6F, little-endian). The sensor must be idle.
    ///
    ///
    /// # Errors
    /// Returns a mode error if runtime mode is not idle, or an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn set_area_lower_bound(
        &mut self,
        x: u16,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(regs::OBJ_AREA_LB_LO, &x.to_le_bytes()).await?;
        Ok(())
    }

    /// Writes the object area upper bound (0x70-0x71, little-endian). The sensor must be idle.
    ///
    ///
    /// # Errors
    /// Returns a mode error if runtime mode is not idle, or an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn set_area_upper_bound(
        &mut self,
        x: u16,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(regs::OBJ_AREA_UB_LO, &x.to_le_bytes()).await?;
        Ok(())
    }

    /// Writes OBJ_LIGHT_THRD (0x6D). The sensor must be idle.
    ///
    ///
    /// # Errors
    /// Returns a mode error if runtime mode is not idle, or an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn set_light_threshold(
        &mut self,
        x: u8,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(regs::OBJ_LIGHT_THRD, &[x]).await?;
        Ok(())
    }

    /// Writes the circle detection ratio bounds. The sensor must be idle.
    ///
    ///
    /// # Errors
    /// Returns a mode error if runtime mode is not idle, or an interface error if register access fails.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn set_circle_params(
        &mut self,
        params: CircleParams,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(regs::CIRCLE_R_LB, &[params.r_lower_bound])
            .await?;
        self.write(regs::CIRCLE_R_UB, &[params.r_upper_bound])
            .await?;
        self.write(regs::CIRCLE_K_LB, &[params.k_lower_bound])
            .await?;
        self.write(regs::CIRCLE_K_UB, &[params.k_upper_bound])
            .await?;
        Ok(())
    }

    /// Writes the ROI registers (0x72-0x76). Bit 8 of each X coordinate goes into ROI_CTRL. The
    /// sensor must be idle.
    /// # Errors
    /// Returns a mode error if runtime mode is not idle, or an interface error if register access fails.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn set_roi(&mut self, roi: Roi) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;

        let ctrl = (roi.enabled as u8)
            | (((roi.x0 >> 8) as u8 & 1) << 4)
            | (((roi.x1 >> 8) as u8 & 1) << 5);

        self.write(regs::ROI_CTRL, &[ctrl]).await?;
        self.write(regs::ROI_X0_LO, &[(roi.x0 & 0xFF) as u8])
            .await?;
        self.write(regs::ROI_Y0, &[roi.y0]).await?;
        self.write(regs::ROI_X1_LO, &[(roi.x1 & 0xFF) as u8])
            .await?;
        self.write(regs::ROI_Y1, &[roi.y1]).await?;
        Ok(())
    }
}

impl<I: Interface, D: DelayNs, M: mode::IsImage> Pag7665Qn<I, D, M> {
    /// Waits on INT_O until a frame is ready, then reads the 320×240 image into `image`.
    /// # Errors
    /// Returns an interface, INT_O wait, or mode error.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn wait_for_image<O: digital::Wait>(
        &mut self,
        interrupt: &mut Pag7665QnInterrupt<O>,
        image: &mut [u8; 320 * 240],
    ) -> Result<(), Error<I::Error, O::Error, M::Error>> {
        self.mode.is_image().map_err(Error::ModeError)?;
        loop {
            interrupt.wait().await.map_err(Error::IntOError)?;
            if self.try_get_image(image).await? {
                return Ok(());
            }
        }
    }

    /// Reads a ready 320×240 frame without waiting for INT_O.
    ///
    /// The report-buffer lock handshake is bounded. Unlock and status cleanup are attempted
    /// even if locking or the image read fails. Callers can poll this with their own deadline
    /// without cancelling an in-progress SPI transaction.
    ///
    /// This keeps the existing 320×240 image-report contract. The encoding of SENSOR_SIZE_SEL
    /// is undocumented here, so other selector values have no verified physical-size mapping.
    /// SPI reads transfer exactly the requested byte count or return an interface error, and
    /// the current interface exposes no actual frame length to validate a different image
    /// size. Do not reinterpret a shorter image as 320×240.
    ///
    /// # Errors
    /// Returns an interface error, mode error, or [`Error::Timeout`] if report-buffer locking times out.
    /// Since: Odyssey Desktop 0.7.0.
    pub async fn try_get_image<EO>(
        &mut self,
        image: &mut [u8; 320 * 240],
    ) -> Result<bool, Error<I::Error, EO, M::Error>> {
        self.mode.is_image().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        if !self.int_o_status().await?.frame_ready() {
            return Ok(false);
        }
        let result: Result<bool, Error<I::Error, EO, M::Error>> = async {
            self.write(regs::REPORT_BUF_SEL, &[1]).await?;
            for _ in 0..HANDSHAKE_POLLS {
                if self.read_byte(regs::REPORT_BUF_SEL).await? != 0 {
                    self.read(regs::RPT_SRAM_DATA, image).await?;
                    return Ok(true);
                }
                self.timer.delay_ms(1).await;
            }
            Err(Error::Timeout)
        }
        .await;
        let unlock: Result<(), Error<I::Error, EO, M::Error>> =
            self.write(regs::REPORT_BUF_SEL, &[0]).await;
        let clear: Result<(), Error<I::Error, EO, M::Error>> =
            self.set_int_o_status(IntOStatus::ZERO).await;
        let ready = result?;
        unlock?;
        clear?;
        Ok(ready)
    }
}

impl<I: Interface, D: DelayNs, M: mode::IsObject> Pag7665Qn<I, D, M> {
    /// Waits on INT_O until a frame of object data is ready, fills `obj` and returns the number
    /// of objects detected.
    ///
    /// # Errors
    /// Returns an interface error, INT_O wait error, or mode error.
    /// Also returns [`Error::Timeout`] if object report handshake times out.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn wait_for_objects<O: digital::Wait>(
        &mut self,
        interrupt: &mut Pag7665QnInterrupt<O>,
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
    /// `Ok(None)` if no object data was ready. Use [`Pag7665QnInterrupt::wait`] to wait until a
    /// frame is ready.
    ///
    /// # Errors
    /// Returns an interface error, mode error, [`Error::InvalidObjectCount`] for counts above 16, or [`Error::Timeout`] if report handshake times out.
    ///
    /// Since: Odyssey Desktop 0.6.0.
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
        let mut polls = 0;
        loop {
            let status = self.int_o_status().await?;
            if status.frame_ready() {
                let count = self.read_byte(regs::OBJ_COUNT).await?;
                if count > 16 {
                    self.set_int_o_status(IntOStatus::ZERO).await?;
                    return Err(Error::InvalidObjectCount(count));
                }

                // At most 8 objects are read per report; above 8, the rest arrive in a second
                // report.
                if count > 8 {
                    total_count = count;
                    self.read(regs::OBJ_DATA_START, &mut obj_bytes[..8 * 8])
                        .await?;
                    flag = true;
                } else if count > 0 {
                    total_count = count;
                    self.read(regs::OBJ_DATA_START, &mut obj_bytes[..count as usize * 8])
                        .await?;
                    done = true;
                } else if flag {
                    // Read 2nd frame
                    self.read(
                        regs::OBJ_DATA_START,
                        &mut obj_bytes[8 * 8..total_count as usize * 8],
                    )
                    .await?;
                    done = true;
                } else {
                    done = true;
                }
            } else if !flag {
                self.set_int_o_status(IntOStatus::ZERO).await?;
                return Ok(None);
            }
            polls += 1;
            if polls == HANDSHAKE_POLLS {
                self.set_int_o_status(IntOStatus::ZERO).await?;
                return Err(Error::Timeout);
            }
            self.set_int_o_status(IntOStatus::ZERO).await?;
            if flag && !done {
                self.timer.delay_ms(1).await;
            }
            if done {
                break;
            }
        }
        Ok(Some(total_count))
    }
}

impl<I: Interface, D: DelayNs> Pag7665Qn<I, D, Mode> {
    /// Switches the sensor mode and updates this driver's mode.
    ///
    /// # Errors
    /// Returns an interface error or [`Error::Timeout`] if the sensor does not acknowledge the mode.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn switch_mode_mut(
        &mut self,
        new_mode: Mode,
    ) -> Result<(), Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        // Read the hardware mode, not just the cache: a previous transition may have failed
        // after the idle command was accepted.
        let current_mode = self.read_byte(regs::OP_MODE_ACTIVE).await?;
        if current_mode != new_mode as u8 {
            if current_mode != Mode::Idle as u8 && new_mode != Mode::Idle {
                self.write_mode_and_wait(Mode::Idle).await?;
                self.mode = Mode::Idle;
            }
            self.write_mode_and_wait(new_mode).await?;
        }
        self.mode = new_mode;
        Ok(())
    }

    /// Releases a possibly locked image report and returns to object mode.
    ///
    /// Cleanup ignores the cached mode on purpose, so a cancelled or failed image transition
    /// cannot turn restoration into a no-op. Mode restoration is attempted even when
    /// releasing the report buffer fails.
    ///
    /// Returns an interface error or [`Error::Timeout`] if the sensor does not acknowledge object mode.
    /// Since: Odyssey Desktop 0.7.0.
    pub async fn restore_object_mode(
        &mut self,
    ) -> Result<(), Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let unlock: Result<(), Error<I::Error, Infallible, Infallible>> =
            self.write(regs::REPORT_BUF_SEL, &[0]).await;
        let clear = self.set_int_o_status(IntOStatus::ZERO).await;
        let restore = self.switch_mode_mut(Mode::Object).await;
        restore?;
        unlock?;
        clear
    }
}

impl<O: digital::Wait> Pag7665QnInterrupt<O> {
    /// Waits for INT_O to go high.
    ///
    /// # Errors
    ///
    /// Returns the pin wait error.
    /// Since: Odyssey Desktop 0.6.0.
    pub async fn wait(&mut self) -> Result<(), O::Error> {
        self.int_o.wait_for_high().await
    }
}

/// Since: Odyssey Desktop 0.6.0.
#[expect(
    async_fn_in_trait,
    reason = "the driver interface is asynchronous and has one concrete hardware implementation"
)]
pub trait Interface {
    /// Since: Odyssey Desktop 0.6.0.
    type Error;
    /// Since: Odyssey Desktop 0.6.0.
    /// Writes `data` to the register.
    ///
    /// # Errors
    /// Returns the interface error if the register write fails.
    async fn write(&mut self, address: u8, data: &[u8]) -> Result<(), Self::Error>;
    /// Since: Odyssey Desktop 0.6.0.
    /// Reads register bytes into `data`.
    ///
    /// # Errors
    /// Returns the interface error if the register read fails.
    async fn read(&mut self, address: u8, data: &mut [u8]) -> Result<(), Self::Error>;

    /// Reads one byte from a register.
    ///
    /// # Errors
    /// Returns the interface error if the register read fails.
    /// Since: Odyssey Desktop 0.6.0.
    async fn read_byte(&mut self, address: u8) -> Result<u8, Self::Error> {
        let mut value = [0];
        self.read(address, &mut value).await?;
        Ok(value[0])
    }

    /// Reads INTO_Status (0x04) and logs its error bit. The caller must have selected bank 0x00.
    /// # Errors
    /// Returns the interface error if the status read fails.
    ///
    /// Since: Odyssey Desktop 0.6.0.
    async fn int_o_status(&mut self) -> Result<IntOStatus, Self::Error> {
        let value = self.read_byte(regs::INTO_STATUS).await?;
        let status = IntOStatus::new_with_raw_value(value);
        if status.error() {
            defmt::error!("INTO_Error");
        }
        Ok(status)
    }

    /// Writes INTO_Status (0x04). The caller must have selected bank 0x00.
    ///
    /// # Errors
    /// Returns the interface error if the status write fails.
    /// Since: Odyssey Desktop 0.6.0.
    async fn set_int_o_status(&mut self, value: IntOStatus) -> Result<(), Self::Error> {
        self.write(regs::INTO_STATUS, &[value.raw_value()]).await?;
        Ok(())
    }
}

/// Since: Odyssey Desktop 0.6.0.
#[derive(Debug)]
pub enum InitError<E> {
    /// Since: Odyssey Desktop 0.6.0.
    FailedPartIdCheck,
    /// Since: Odyssey Desktop 0.6.0.
    FailedBootLoader,
    /// Since: Odyssey Desktop 0.6.0.
    InterfaceError(E),
}

impl<E> From<E> for InitError<E> {
    fn from(t: E) -> Self {
        InitError::InterfaceError(t)
    }
}

/// Since: Odyssey Desktop 0.6.0.
#[derive(Debug)]
pub enum Error<I, O, M> {
    /// Since: Odyssey Desktop 0.6.0.
    InterfaceError(I),
    /// Since: Odyssey Desktop 0.6.0.
    IntOError(O),
    /// Since: Odyssey Desktop 0.6.0.
    ModeError(M),
    /// The sensor reported more objects than its 16-object output buffer can hold.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    InvalidObjectCount(u8),
    /// The sensor did not acknowledge a mode/report handshake within 100 ms.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    Timeout,
}

#[cfg(test)]
mod tests {
    use core::task::{Context, Poll, Waker};

    use super::*;

    struct ReadyDelay {
        polls: usize,
    }

    impl DelayNs for ReadyDelay {
        async fn delay_ns(&mut self, _: u32) {
            self.polls += 1;
        }
    }

    struct Sensor {
        active: u8,
        refuse_image: bool,
        refuse_object: bool,
        refuse_lock: bool,
        fail_image_read: bool,
        locked: bool,
        frame_ready: bool,
    }

    impl Interface for Sensor {
        type Error = ();

        async fn write(&mut self, address: u8, data: &[u8]) -> Result<(), ()> {
            match address {
                regs::OP_MODE_HOST => {
                    if !(data[0] == Mode::Image as u8 && self.refuse_image)
                        && !(data[0] == Mode::Object as u8 && self.refuse_object)
                    {
                        self.active = data[0];
                    }
                }
                regs::REPORT_BUF_SEL => self.locked = data[0] != 0 && !self.refuse_lock,
                regs::INTO_STATUS => self.frame_ready = data[0] & 2 != 0,
                _ => {}
            }
            Ok(())
        }

        async fn read(&mut self, address: u8, data: &mut [u8]) -> Result<(), ()> {
            match address {
                regs::OP_MODE_ACTIVE => data[0] = self.active,
                regs::REPORT_BUF_SEL => data[0] = u8::from(self.locked),
                regs::INTO_STATUS => data[0] = if self.frame_ready { 2 } else { 0 },
                regs::RPT_SRAM_DATA if self.fail_image_read => return Err(()),
                _ => data.fill(0),
            }
            Ok(())
        }
    }

    fn sensor(active: Mode) -> Pag7665Qn<Sensor, ReadyDelay, Mode> {
        Pag7665Qn {
            bus: Sensor {
                active: active as u8,
                refuse_image: false,
                refuse_object: false,
                refuse_lock: false,
                fail_image_read: false,
                locked: false,
                frame_ready: true,
            },
            timer: ReadyDelay { polls: 0 },
            mode: active,
            bank: 0,
        }
    }

    fn ready<F: Future>(future: F) -> F::Output {
        let mut future = core::pin::pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("mock sensor unexpectedly suspended"),
        }
    }

    #[test]
    fn failed_image_transition_restores_object_from_actual_idle() {
        let mut driver = sensor(Mode::Object);
        driver.bus.refuse_image = true;
        assert!(matches!(
            ready(driver.switch_mode_mut(Mode::Image)),
            Err(Error::Timeout)
        ));
        assert_eq!(driver.timer.polls, HANDSHAKE_POLLS);
        assert_eq!(driver.bus.active, Mode::Idle as u8);
        ready(driver.restore_object_mode()).unwrap();
        assert_eq!(driver.bus.active, Mode::Object as u8);
        assert!(!driver.bus.locked);
    }

    #[test]
    fn restoration_does_not_trust_stale_object_cache() {
        let mut driver = sensor(Mode::Object);
        driver.bus.active = Mode::Image as u8;
        driver.bus.locked = true;
        ready(driver.restore_object_mode()).unwrap();
        assert_eq!(driver.bus.active, Mode::Object as u8);
        assert!(!driver.bus.locked);
        assert!(!driver.bus.frame_ready);
    }

    #[test]
    fn failed_image_read_releases_report_buffer() {
        let mut driver = sensor(Mode::Image);
        driver.bus.fail_image_read = true;
        let mut image = [0; 320 * 240];
        assert!(matches!(
            ready(driver.try_get_image::<Infallible>(&mut image)),
            Err(Error::InterfaceError(()))
        ));
        assert!(!driver.bus.locked);
        assert!(!driver.bus.frame_ready);
    }

    #[test]
    fn report_lock_failure_is_bounded_and_clears_ready() {
        let mut driver = sensor(Mode::Image);
        driver.bus.refuse_lock = true;
        let mut image = [0; 320 * 240];
        assert!(matches!(
            ready(driver.try_get_image::<Infallible>(&mut image)),
            Err(Error::Timeout)
        ));
        assert_eq!(driver.timer.polls, HANDSHAKE_POLLS);
        assert!(!driver.bus.locked);
        assert!(!driver.bus.frame_ready);
    }
}
