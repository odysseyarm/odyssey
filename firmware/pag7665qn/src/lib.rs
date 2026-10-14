#![no_std]
#![allow(dead_code)]

use core::{convert::Infallible, future::Future};

use bytemuck::bytes_of_mut;
use embedded_hal_async::{delay::DelayNs, digital, spi::SpiDevice};

pub mod mode;
pub mod spi;
pub mod types;

use mode::{Mode, ModeT};
use spi::Pag7665QnSpi;
use types::{CircleParams, IntOStatus, Roi};

// PAG7665QN Register addresses (Bank 0x00 unless noted)
mod regs {
    // Identification
    pub const PART_ID_LO: u8 = 0x00;
    pub const PART_ID_HI: u8 = 0x01;

    // Status and control
    pub const INTO_STATUS: u8 = 0x04;
    pub const CPU_RESET_ENL: u8 = 0x0A;
    pub const REPORT_BUF_SEL: u8 = 0x0E;
    pub const RPT_SRAM_DATA: u8 = 0x0F;

    // Mode control
    pub const OP_MODE_HOST: u8 = 0x10;
    pub const SENSOR_SIZE_SEL: u8 = 0x12;
    pub const SENSOR_FRAME_RATE: u8 = 0x13;
    pub const OP_MODE_ACTIVE: u8 = 0x14;

    // Firmware version
    pub const FW_MAJOR_VERSION: u8 = 0x15;
    pub const FW_MINOR_VERSION: u8 = 0x7C;
    pub const FW_CUSTOMER_NUM: u8 = 0x7D;
    pub const FW_REVISION_NUM: u8 = 0x7E;

    // Circle detection parameters
    pub const CIRCLE_R_LB: u8 = 0x23;
    pub const CIRCLE_R_UB: u8 = 0x24;
    pub const CIRCLE_K_LB: u8 = 0x77;
    pub const CIRCLE_K_UB: u8 = 0x78;

    // Object output
    pub const OBJ_COUNT: u8 = 0x25;
    pub const OBJ_DATA_START: u8 = 0x26; // 8 bytes per object, 8 objects max per read

    // Manual power control
    pub const MANUAL_POWER_CTRL: u8 = 0x2F;
    pub const MANUAL_POWER_TRIG: u8 = 0x30;

    // Sensor settings (PAG7665QN specific addresses)
    pub const SENSOR_EXPO: u8 = 0x67; // Exposure: bit[7]=LED_Always_ON, bits[6:0]=time
    pub const SENSOR_GAIN: u8 = 0x68; // Analog gain
    pub const OBJ_LIGHT_THRD: u8 = 0x6D; // Light threshold
    pub const OBJ_AREA_LB_LO: u8 = 0x6E; // Area lower bound [7:0]
    pub const OBJ_AREA_LB_HI: u8 = 0x6F; // Area lower bound [15:8]
    pub const OBJ_AREA_UB_LO: u8 = 0x70; // Area upper bound [7:0]
    pub const OBJ_AREA_UB_HI: u8 = 0x71; // Area upper bound [15:8]

    // ROI (Region of Interest)
    pub const ROI_CTRL: u8 = 0x72; // bit[5]=X1[8], bit[4]=X0[8], bit[0]=ROI_EN
    pub const ROI_X0_LO: u8 = 0x73; // X0[7:0]
    pub const ROI_Y0: u8 = 0x74; // Y0[7:0]
    pub const ROI_X1_LO: u8 = 0x75; // X1[7:0]
    pub const ROI_Y1: u8 = 0x76; // Y1[7:0]

    // Error registers
    pub const FW_ERROR_TYPE: u8 = 0x7A;
    pub const FW_ERROR_CODE: u8 = 0x7B;

    // Bank switching (non-bank register)
    pub const BANK_NO: u8 = 0x7F;

    // Bank 0x06 registers
    pub mod bank6 {
        pub const SW_RESET: u8 = 0x1F;
    }
}

/// PAG7665QN driver
pub struct Pag7665Qn<I, D, M> {
    bus: I,
    timer: D,
    mode: M,
    bank: u8,
}

/// PAG7665QN interrupt pin
pub struct Pag7665QnInterrupt<O> {
    pub int_o: O,
}

impl<S: SpiDevice, D: DelayNs, M: ModeT> Pag7665Qn<Pag7665QnSpi<S>, D, M> {
    /// Creates a new PAG7665QN driver using SPI and perform initialization.
    ///
    /// Make sure `spi` is configured in mode 3 with MSB first bit order, max 20 MHz.
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

    // Read Part ID - PAG7665QN should return 0x7660
    let mut part_id = [0; 2];
    bus.read(regs::PART_ID_LO, &mut part_id).await?;
    if part_id != [0x60, 0x76] {
        defmt::info!(
            "Part ID check failed, got 0x{=u8:X}{=u8:X}, retrying",
            part_id[1],
            part_id[0]
        );
        // Try again
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

    // Set initial mode and enable CPU
    bus.write(regs::OP_MODE_HOST, &[initial_mode as u8]).await?;
    bus.write(regs::CPU_RESET_ENL, &[1]).await?;

    // Wait for power-on ready (up to 300ms per datasheet)
    timer.delay_ms(250).await;
    let mut int_o_status = [0];
    bus.read(regs::INTO_STATUS, &mut int_o_status).await?;
    if int_o_status[0] & 1 != 1 {
        return Err(InitError::FailedBootLoader);
    }

    // Clear status
    bus.write(regs::INTO_STATUS, &[0]).await?;

    // Read and log firmware version
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
    pub fn set_mode_unchecked<N: ModeT>(self, mode: N) -> Pag7665Qn<I, D, N> {
        Pag7665Qn {
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
        self.write(regs::BANK_NO, &[bank]).await?;
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
    ) -> Result<Pag7665Qn<I, D, N>, Error<I::Error, Infallible, Infallible>> {
        if new_mode.mode() != self.mode.mode() {
            defmt::debug!("Switching mode to {}", new_mode.mode());
            self._set_bank(0x00).await?;
            if self.mode.mode() != Mode::Idle && new_mode.mode() != Mode::Idle {
                // switch to idle first
                self.write(regs::OP_MODE_HOST, &[Mode::Idle as u8]).await?;
                loop {
                    let mut current_mode = [0];
                    self.read(regs::OP_MODE_ACTIVE, &mut current_mode).await?;
                    if current_mode[0] == Mode::Idle as u8 {
                        break;
                    }
                }
            }

            self.write(regs::OP_MODE_HOST, &[new_mode.mode() as u8])
                .await?;
            loop {
                let mut current_mode = [0];
                self.read(regs::OP_MODE_ACTIVE, &mut current_mode).await?;
                if current_mode[0] == new_mode.mode() as u8 {
                    break;
                }
            }
            defmt::debug!("Switched mode to {}", new_mode.mode());
        }
        Ok(Pag7665Qn {
            mode: new_mode,
            bus: self.bus,
            timer: self.timer,
            bank: self.bank,
        })
    }

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

    /// Get the sensor FPS.
    pub async fn sensor_fps(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let mut value = [0];
        self.read(regs::SENSOR_FRAME_RATE, &mut value).await?;
        Ok(value[0])
    }

    /// Get the sensor's exposure time and led mode.
    pub async fn sensor_exposure_us(
        &mut self,
    ) -> Result<(bool, u16), Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let value = self.read_byte(regs::SENSOR_EXPO).await?;
        let led_always_on = value & (1 << 7) != 0;
        let exposure_us = u16::from(value & !(1 << 7)) * 100;
        Ok((led_always_on, exposure_us))
    }

    /// Get the sensor gain.
    pub async fn sensor_gain(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        self.read_byte(regs::SENSOR_GAIN).await
    }

    /// Get the area lower bound
    pub async fn area_lower_bound(
        &mut self,
    ) -> Result<u16, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let mut value = 0u16;
        self.read(regs::OBJ_AREA_LB_LO, bytes_of_mut(&mut value))
            .await?;
        Ok(value)
    }

    /// Get the area upper bound
    pub async fn area_upper_bound(
        &mut self,
    ) -> Result<u16, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        let mut value = 0u16;
        self.read(regs::OBJ_AREA_UB_LO, bytes_of_mut(&mut value))
            .await?;
        Ok(value)
    }

    /// Get the light threshold
    pub async fn light_threshold(&mut self) -> Result<u8, Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x00).await?;
        self.read_byte(regs::OBJ_LIGHT_THRD).await
    }

    /// Get circle detection parameters
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

    /// Get ROI configuration
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

    /// Perform a software reset. All registers will be cleared.
    pub async fn software_reset(&mut self) -> Result<(), Error<I::Error, Infallible, Infallible>> {
        self._set_bank(0x06).await?;
        self.write(regs::bank6::SW_RESET, &[0xFF]).await?;
        self.timer.delay_ms(1).await;
        self.bank = 0; // Reset bank state
        Ok(())
    }
}

impl<I: Interface, D: DelayNs, M: mode::IsIdle> Pag7665Qn<I, D, M> {
    /// Set the sensor FPS.
    pub async fn set_sensor_fps(
        &mut self,
        value: u8,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(regs::SENSOR_FRAME_RATE, &[value]).await?;
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
        self.write(regs::SENSOR_EXPO, &[value]).await?;
        Ok(())
    }

    /// Set the sensor gain.
    pub async fn set_sensor_gain(
        &mut self,
        x: u8,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(regs::SENSOR_GAIN, &[x]).await?;
        Ok(())
    }

    /// Set the area lower bound
    pub async fn set_area_lower_bound(
        &mut self,
        x: u16,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(regs::OBJ_AREA_LB_LO, &x.to_le_bytes()).await?;
        Ok(())
    }

    /// Set the area upper bound
    pub async fn set_area_upper_bound(
        &mut self,
        x: u16,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(regs::OBJ_AREA_UB_LO, &x.to_le_bytes()).await?;
        Ok(())
    }

    /// Set the light threshold
    pub async fn set_light_threshold(
        &mut self,
        x: u8,
    ) -> Result<(), Error<I::Error, Infallible, M::Error>> {
        self.mode.is_idle().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;
        self.write(regs::OBJ_LIGHT_THRD, &[x]).await?;
        Ok(())
    }

    /// Set circle detection parameters
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

    /// Set ROI configuration
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
    /// Wait for and get a frame of image data.
    pub async fn wait_for_image<O: digital::Wait>(
        &mut self,
        interrupt: &mut Pag7665QnInterrupt<O>,
        image: &mut [u8; 320 * 240],
    ) -> Result<(), Error<I::Error, O::Error, M::Error>> {
        self.mode.is_image().map_err(Error::ModeError)?;
        self._set_bank(0x00).await?;

        loop {
            interrupt.wait().await.map_err(Error::IntOError)?;
            let status = self.int_o_status().await?;
            if status.frame_ready() {
                // Lock frame buffer
                self.write(regs::REPORT_BUF_SEL, &[1]).await?;
                // Read-back check
                while self.read_byte(regs::REPORT_BUF_SEL).await? == 0 {}

                // Read image
                self.read(regs::RPT_SRAM_DATA, image).await?;

                // Unlock frame buffer
                self.write(regs::REPORT_BUF_SEL, &[0]).await?;
                // Clear frame ready
                self.set_int_o_status(IntOStatus::ZERO).await?;
                break;
            }
        }
        Ok(())
    }
}

impl<I: Interface, D: DelayNs, M: mode::IsObject> Pag7665Qn<I, D, M> {
    /// Wait for and get a frame of object data. Returns the number of objects that were detected.
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
                let count = self.read_byte(regs::OBJ_COUNT).await?;

                // Read object output
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
            self.set_int_o_status(IntOStatus::ZERO).await?;
            if done {
                break;
            }
        }
        Ok(Some(total_count))
    }
}

impl<I: Interface, D: DelayNs> Pag7665Qn<I, D, Mode> {
    pub async fn switch_mode_mut(
        &mut self,
        new_mode: Mode,
    ) -> Result<(), Error<I::Error, Infallible, Infallible>> {
        if new_mode.mode() != self.mode.mode() {
            defmt::debug!("Switching mode to {}", new_mode.mode());
            self._set_bank(0x00).await?;
            if self.mode.mode() != Mode::Idle && new_mode.mode() != Mode::Idle {
                // switch to idle first
                self.write(regs::OP_MODE_HOST, &[Mode::Idle as u8]).await?;
                loop {
                    let mut current_mode = [0];
                    self.read(regs::OP_MODE_ACTIVE, &mut current_mode).await?;
                    if current_mode[0] == Mode::Idle as u8 {
                        break;
                    }
                }
            }

            self.write(regs::OP_MODE_HOST, &[new_mode.mode() as u8])
                .await?;
            loop {
                let mut current_mode = [0];
                self.read(regs::OP_MODE_ACTIVE, &mut current_mode).await?;
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

impl<O: digital::Wait> Pag7665QnInterrupt<O> {
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
        let value = self.read_byte(regs::INTO_STATUS).await?;
        let status = IntOStatus::new_with_raw_value(value);
        if status.error() {
            defmt::error!("INTO_Error");
        }
        Ok(status)
    }

    /// Set the value of the INTO_Status register. Make sure the bank is set to 0x00.
    async fn set_int_o_status(&mut self, value: IntOStatus) -> Result<(), Self::Error> {
        self.write(regs::INTO_STATUS, &[value.raw_value()]).await?;
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
