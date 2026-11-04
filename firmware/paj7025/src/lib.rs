#![no_std]

//! Portable PAJ7025 register access and object/image configuration.
//!
//! Multi-register configuration is not cancellation safe. Cancellation or an SPI error can leave
//! the sensor partially configured; releasing the SPI device does not reset the sensor.
//!
//! Image-mode sequences come from the last pre-deletion Nordic driver:
//! <https://github.com/odysseyarm/donger/blob/a99890d13d11702a6cd5f390a7b88aeb2be33882/paj7025-nrf/src/lib.rs>.
//!
//! The recovered sequence enters sensor-master image output, not a reversible
//! register-selected capture mode. No image-master exit procedure is present in
//! that source. The public PAJ7025R2 pin table lists no hardware reset pin:
//! <https://www.alldatasheet.com/html-pdf/987645/PIXART/PAJ7025R2/1114/5/PAJ7025R2.html>.
//! Its bank-0 software reset (0x64) still requires register access:
//! <https://www.alldatasheet.com/html-pdf/987645/PIXART/PAJ7025R2/2234/10/PAJ7025R2.html>.
//! Image capture is one-way until the board's reboot/power-cycle recovery workflow;
//! stopping reception must never recreate a register SPI master on these pins.

pub mod types;
mod u2048;

#[cfg(test)]
mod tests;

use device_driver::AsyncRegisterInterface;
use embedded_hal_async::spi::Operation;

use crate::types::ObjectFormat1;

const REG_BANK_SELECT: u8 = 0xEF; // Non-bank register for bank switching
const CMD_MULTI: u8 = 0x01; // CMD[6:0] = 0x00 single, 0x01 multi
const CMD_READ: u8 = 0x80; // CMD bit7 = 1 => Read
const CMD_WRITE: u8 = 0x00; // CMD bit7 = 0 => Write
const IMAGE_EXPOSURE_STEP_NS: u32 = 800;
const MIN_IMAGE_EXPOSURE_NS: u32 = 20_000;
const MAX_IMAGE_EXPOSURE_NS: u32 = u16::MAX as u32 * IMAGE_EXPOSURE_STEP_NS;

#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Paj7025Error<SpiError> {
    Spi(SpiError),
    /// Image exposure must be 20,000..=52,428,000 ns, in exact 800 ns steps.
    InvalidImageExposure {
        nanoseconds: u32,
    },
}

pub struct DeviceInterface<SpiDevice> {
    spi_device: Option<SpiDevice>,
}

impl<I> DeviceInterface<I> {
    pub fn new(spi_device: I) -> Self {
        Self {
            spi_device: Some(spi_device),
        }
    }

    async fn write_current_bank(
        &mut self,
        register: u8,
        data: &[u8],
    ) -> Result<(), Paj7025Error<I::Error>>
    where
        I: embedded_hal_async::spi::SpiDevice,
    {
        let cmd = CMD_WRITE | if data.len() > 1 { CMD_MULTI } else { 0 };
        let cmd_addr = [cmd, register];
        self.spi_device
            .as_mut()
            .expect("sensor released for image mode")
            .transaction(&mut [Operation::Write(&cmd_addr), Operation::Write(data)])
            .await
            .map_err(Paj7025Error::Spi)
    }
}

impl<I, E> AsyncRegisterInterface for DeviceInterface<I>
where
    I: embedded_hal_async::spi::SpiDevice<Error = E>,
    E: core::fmt::Debug,
{
    type AddressType = u16;
    type Error = Paj7025Error<E>;

    async fn read_register(
        &mut self,
        address: u16,
        _size_bits: u32,
        data: &mut [u8],
    ) -> Result<(), Self::Error> {
        let bank: u8 = (address >> 8) as u8;
        let reg: u8 = (address & 0xFF) as u8;

        let cmd_bank = [CMD_WRITE | 0x00, REG_BANK_SELECT];
        let bank_val = [bank];
        let mut ops1 = [Operation::Write(&cmd_bank), Operation::Write(&bank_val)];
        self.spi_device
            .as_mut()
            .expect("sensor released for image mode")
            .transaction(&mut ops1)
            .await
            .map_err(Paj7025Error::Spi)?;

        let cmd = CMD_READ | if data.len() > 1 { CMD_MULTI } else { 0 };
        let cmd_addr = [cmd, reg];
        let mut ops2 = [
            Operation::Write(&cmd_addr),
            Operation::TransferInPlace(data),
        ];
        self.spi_device
            .as_mut()
            .expect("sensor released for image mode")
            .transaction(&mut ops2)
            .await
            .map_err(Paj7025Error::Spi)?;
        Ok(())
    }

    async fn write_register(
        &mut self,
        address: u16,
        _size_bits: u32,
        data: &[u8],
    ) -> Result<(), Self::Error> {
        let bank: u8 = (address >> 8) as u8;
        let reg: u8 = (address & 0xFF) as u8;

        let cmd_bank = [CMD_WRITE | 0x00, REG_BANK_SELECT];
        let bank_val = [bank];
        let mut ops1 = [Operation::Write(&cmd_bank), Operation::Write(&bank_val)];
        self.spi_device
            .as_mut()
            .expect("sensor released for image mode")
            .transaction(&mut ops1)
            .await
            .map_err(Paj7025Error::Spi)?;

        self.write_current_bank(reg, data).await
    }
}

pub struct Paj7025<SpiDevice, E>
where
    SpiDevice: embedded_hal_async::spi::SpiDevice<Error = E>,
    E: core::fmt::Debug,
{
    pub ll: low_level::Paj7025<DeviceInterface<SpiDevice>>,
    _marker: core::marker::PhantomData<E>,
}

impl<SpiDevice, E> Paj7025<SpiDevice, E>
where
    SpiDevice: embedded_hal_async::spi::SpiDevice<Error = E>,
    E: core::fmt::Debug,
{
    pub async fn init(spi_device: SpiDevice) -> Result<Self, Paj7025Error<E>> {
        let driver = Self {
            ll: low_level::Paj7025::new(DeviceInterface::new(spi_device)),
            _marker: core::marker::PhantomData,
        };

        Ok(driver)
    }

    pub async fn read_register(
        &mut self,
        bank: u8,
        address: u8,
        data: &mut [u8],
    ) -> Result<(), Paj7025Error<E>> {
        let address = ((bank as u16) << 8) | address as u16;
        self.ll.interface().read_register(address, 0, data).await
    }

    pub async fn write_register(
        &mut self,
        bank: u8,
        address: u8,
        data: &[u8],
    ) -> Result<(), Paj7025Error<E>> {
        let address = ((bank as u16) << 8) | address as u16;
        self.ll.interface().write_register(address, 0, data).await
    }
}

impl<SpiDevice, E> Paj7025<SpiDevice, E>
where
    SpiDevice: embedded_hal_async::spi::SpiDevice<Error = E>,
    E: core::fmt::Debug,
{
    /// Apply the legacy calibration startup settings, with G6 in STREAM mode.
    ///
    /// Unlike [`Self::init_settings`], this does not select object operation mode, configure
    /// object gains/exposure, or assign G6 to FOD. The caller must power the sensor and check
    /// its product ID before calling this method. An error or cancellation leaves partial settings.
    ///
    /// # Errors
    /// Returns the SPI error if any configuration write fails.
    pub async fn init_image_settings(&mut self, v_flip: bool) -> Result<(), Paj7025Error<E>> {
        let writes = [
            (0xEF, 0x00),
            (0xDC, 0x00),
            (0xFB, 0x04),
            (0xEF, 0x00),
            (0x2F, 0x05),
            (0x30, 0x00),
            (0x30, 0x01),
            (0x1F, 0x00),
            (0xEF, 0x01),
            (0x2D, u8::from(v_flip)),
            (0xEF, 0x0C),
            (0x64, 0x00),
            (0x65, 0x00),
            (0x66, 0x00),
            (0x67, 0x00),
            (0x68, 0x00),
            (0x69, 0x00),
            (0x6A, 0x00),
            (0x6B, 0x00),
            (0x6C, 0x00),
            (0x71, 0x00),
            (0x72, 0x00),
            (0x12, 0x00),
            (0x13, 0x00),
            (0xEF, 0x00),
            (0x01, 0x01),
        ];
        for (register, value) in writes {
            self.ll
                .interface()
                .write_current_bank(register, &[value])
                .await?;
        }
        Ok(())
    }

    /// Set image-mode exposure in nanoseconds, independently of object-mode exposure units.
    ///
    /// The legacy image path uses 800 ns per raw count (not the object path's 200 ns).
    /// Values below 20,000 ns, above 52,428,000 ns, or not divisible by 800 are rejected
    /// before any SPI access. This writes bank C registers 0x0F..=0x10; the caller remains
    /// responsible for the legacy bank-1 settings synchronization before entering image mode.
    /// SPI errors or cancellation can leave the exposure partially written.
    ///
    /// # Errors
    /// Returns [`Paj7025Error::InvalidImageExposure`] for an unrepresentable exposure,
    /// or the SPI error if writing a valid exposure fails.
    pub async fn set_image_exposure_ns(&mut self, nanoseconds: u32) -> Result<(), Paj7025Error<E>> {
        if !(MIN_IMAGE_EXPOSURE_NS..=MAX_IMAGE_EXPOSURE_NS).contains(&nanoseconds)
            || !nanoseconds.is_multiple_of(IMAGE_EXPOSURE_STEP_NS)
        {
            return Err(Paj7025Error::InvalidImageExposure { nanoseconds });
        }
        let raw = (nanoseconds / IMAGE_EXPOSURE_STEP_NS) as u16;
        self.write_register(0x0C, 0x0F, &raw.to_le_bytes()).await
    }

    /// Enter image streaming mode and release the owned SPI device and its pin borrows.
    ///
    /// Consumes the driver on both success and SPI error so the caller can reassign the pins
    /// to an image receiver. Dropping the pending future also releases the device. Neither
    /// cancellation nor error rolls back the writes: the sensor may be partially transitioned,
    /// and the caller must recover its hardware state before reuse.
    ///
    /// Bank C registers 0x6D..=0x70 reassign G9..G12 (the register SPI pins) to
    /// sensor-master image output. Releasing the host SPI device does not undo that
    /// assignment. Do not recreate a host SPI master on those pins while the sensor
    /// remains in image mode: the sensor may still drive SCK/CS/data. The recovered
    /// driver provides no reverse sequence, and [`Self::init_settings`] is not one.
    /// Recovery requires the board's reboot/power-cycle workflow; stopping image
    /// reception is not a transition back to register mode.
    ///
    /// # Errors
    /// Returns the SPI error if any transition write fails; the device is still released.
    pub async fn into_image_mode(mut self) -> Result<(), Paj7025Error<E>> {
        self.release_for_image_mode().await
    }

    /// Irreversibly enter image output and release register SPI, including on error.
    /// No register methods may be used afterwards. This lets a board retain the
    /// shared driver allocation while handing its peripheral to an image receiver.
    pub async fn release_for_image_mode(&mut self) -> Result<(), Paj7025Error<E>> {
        let writes = [
            (0xEF, 0x00),
            (0xDC, 0x00),
            (0xFB, 0x00),
            (0x7A, 0x07),
            (0x72, 0x03),
            (0x82, 0x03),
            (0x2F, 0x07),
            (0x30, 0x00),
            (0x30, 0x01),
            (0xDC, 0x00),
            (0xEF, 0x0C),
            (0x6D, 0x06),
            (0x6E, 0x06),
            (0x6F, 0x06),
            (0x70, 0x06),
            (0xEF, 0x00),
            (0x01, 0x01),
        ];
        let result = async {
            for (register, value) in writes {
                self.ll
                    .interface()
                    .write_current_bank(register, &[value])
                    .await?;
            }
            Ok(())
        }
        .await;
        self.ll.interface().spi_device.take();
        result
    }

    pub async fn init_settings(
        &mut self,
        v_flip: bool,
        gain_global: u8,
        gain_ggh: u8,
    ) -> Result<(), Paj7025Error<E>> {
        #[inline(always)]
        async fn w<SpiDevice, E>(
            ll: &mut crate::low_level::Paj7025<crate::DeviceInterface<SpiDevice>>,
            bank: u8,
            reg: u8,
            val: u8,
        ) -> Result<(), Paj7025Error<E>>
        where
            SpiDevice: embedded_hal_async::spi::SpiDevice<Error = E>,
            E: core::fmt::Debug,
        {
            let addr = ((bank as u16) << 8) | reg as u16;
            ll.interface()
                .write_register(addr, 0, core::slice::from_ref(&val))
                .await
        }

        #[inline(always)]
        async fn w_multi<SpiDevice, E>(
            ll: &mut crate::low_level::Paj7025<crate::DeviceInterface<SpiDevice>>,
            bank: u8,
            reg: u8,
            data: &[u8],
        ) -> Result<(), Paj7025Error<E>>
        where
            SpiDevice: embedded_hal_async::spi::SpiDevice<Error = E>,
            E: core::fmt::Debug,
        {
            let addr = ((bank as u16) << 8) | reg as u16;
            ll.interface().write_register(addr, 0, data).await
        }

        // Every write selects its bank first; the writes are grouped by bank only for reading.
        w(&mut self.ll, 0x00, 0x12, 0x01).await?; // operation mode
        w(&mut self.ll, 0x00, 0xDC, 0x00).await?; // internal_system_control_disable
        w(&mut self.ll, 0x00, 0xFB, 0x04).await?; // LEDDAC disable
        w(&mut self.ll, 0x00, 0x2F, 0x05).await?; // sensor_on
        w(&mut self.ll, 0x00, 0x30, 0x00).await?; // Manual_PowerControl_Update_Req (0)
        w(&mut self.ll, 0x00, 0x30, 0x01).await?; // Manual_PowerControl_Update_Req (1)
        w(&mut self.ll, 0x00, 0x1F, 0x00).await?; // freerun_irtx_disable

        w(&mut self.ll, 0x01, 0x2D, if v_flip { 0x01 } else { 0x00 }).await?; // V flip

        // Resolution 4095 x 4095, little-endian [0xFF, 0x0F].
        let res_xy = [0xFFu8, 0x0F];
        w_multi(&mut self.ll, 0x0C, 0x60, &res_xy).await?; // scale X
        w_multi(&mut self.ll, 0x0C, 0x62, &res_xy).await?; // scale Y

        // Gx modes
        w(&mut self.ll, 0x0C, 0x64, 0x00).await?; // G0
        w(&mut self.ll, 0x0C, 0x65, 0x00).await?; // G1
        w(&mut self.ll, 0x0C, 0x66, 0x00).await?; // G2
        w(&mut self.ll, 0x0C, 0x67, 0x00).await?; // G3
        w(&mut self.ll, 0x0C, 0x68, 0x00).await?; // G4
        w(&mut self.ll, 0x0C, 0x69, 0x00).await?; // G5
        w(&mut self.ll, 0x0C, 0x6A, 0x07).await?; // G6 = FOD (0x00 would be STREAM)
        w(&mut self.ll, 0x0C, 0x6B, 0x00).await?; // G7
        w(&mut self.ll, 0x0C, 0x6C, 0x00).await?; // G8
        w(&mut self.ll, 0x0C, 0x71, 0x00).await?; // G13
        w(&mut self.ll, 0x0C, 0x72, 0x00).await?; // G14

        // keyscan disable
        w(&mut self.ll, 0x0C, 0x12, 0x00).await?;
        w(&mut self.ll, 0x0C, 0x13, 0x00).await?;

        // Coordinates stay invalid until Bank 0 is synced.
        w(&mut self.ll, 0x00, 0x01, 0x01).await?; // Bank0_Sync_Updated_Flag

        w(&mut self.ll, 0x0C, 0x0B, gain_global).await?; // B_global
        w(&mut self.ll, 0x0C, 0x0C, gain_ggh).await?; // B_ggh

        // Exposure 0x2000 (units 200ns) split as in C: 0x0F=0x00, 0x10=0x20
        w(&mut self.ll, 0x0C, 0x0F, 0x00).await?; // Texp LSB
        w(&mut self.ll, 0x0C, 0x10, 0x20).await?; // Texp MSB

        // DSP thresholds
        w(&mut self.ll, 0x0C, 0x46, 0x00).await?; // oalb
        w(&mut self.ll, 0x0C, 0x47, 0x6E).await?; // thd (Yth = 110)

        w(&mut self.ll, 0x01, 0x01, 0x01).await?; // Bank1_Sync_Updated_Flag

        Ok(())
    }
}

pub mod low_level {
    #[allow(non_camel_case_types)]
    type u2048 = crate::u2048::U2048x;
    device_driver::create_device!(
        device_name: Paj7025,
        dsl: {
            config {
                type DefaultRegisterAccess = RW;
                type DefaultFieldAccess = RW;
                type DefaultBufferAccess = RW;
                type DefaultByteOrder = LE;
                type DefaultBitOrder = LSB0;
                type RegisterAddressType = u16; // [bank][address]
                type BufferAddressType = u16;
                type NameWordBoundaries = [
                    Underscore, Hyphen, Space, LowerUpper,
                    UpperDigit, DigitUpper, DigitLower,
                    LowerDigit, Acronym,
                ];
                type DefmtFeature = "defmt";
            }
            block Control {
                block Bank0 {
                    register Bank0_Sync_Updated_Flag {
                        type Access = WO;
                        const ADDRESS = 0x01;
                        const SIZE_BITS = 1;
                        value: uint = 0..1,
                    },
                    register Product_ID {
                        type Access = RO;
                        const ADDRESS = 0x02;
                        const SIZE_BITS = 16;
                        value: uint = 0..16,
                    },
                    #[doc = "DSP settings; area max threshold"]
                    register Cmd_oahb {
                        const ADDRESS = 0x0B;
                        const SIZE_BITS = 14;
                        value: uint = 0..14,
                    },
                    #[doc = "DSP settings; noise threshold"]
                    register Cmd_nthd {
                        const ADDRESS = 0x0F;
                        const SIZE_BITS = 8;
                        value: uint = 0..8,
                    },
                    register Cmd_dsp_operation_mode {
                        const ADDRESS = 0x12;
                        const SIZE_BITS = 8;
                        value: uint = 0..8,
                    },
                    register Cmd_max_object_num {
                        const ADDRESS = 0x19;
                        const SIZE_BITS = 5;
                        value: uint = 0..5,
                    },
                    register Cmd_FrameSubtraction_On {
                        const ADDRESS = 0x28;
                        const SIZE_BITS = 8;
                        value: uint = 0..8,
                    },
                },
                block Bank1 {
                    const ADDRESS_OFFSET = 0x0100;
                    register Bank1_Sync_Updated_Flag {
                        type Access = WO;
                        const ADDRESS = 0x01;
                        const SIZE_BITS = 1;
                        value: uint = 0..1,
                    },
                    #[doc = "Read sensor gain 1"]
                    register B_global_R {
                        type Access = RO;
                        const ADDRESS = 0x05;
                        const SIZE_BITS = 5;
                        value: uint = 0..5,
                    },
                    #[doc = "Read sensor gain 2"]
                    register B_ggh_R {
                        type Access = RO;
                        const ADDRESS = 0x06;
                        const SIZE_BITS = 2;
                        value: uint = 0..2,
                    },
                    /// Read exposure time
                    /// unit = 200ns. The minimum setting is 100.
                    register B_expo_R {
                        type Access = RO;
                        const ADDRESS = 0x0E;
                        const SIZE_BITS = 16;
                        value: uint = 0..16,
                    },
                },
                block BankC {
                    const ADDRESS_OFFSET = 0x0C00;
                    #[doc = "Frame period; unit = 100ns"]
                    register Cmd_frame_period {
                        const ADDRESS = 0x07;
                        const SIZE_BITS = 24;
                        value: uint = 0..24,
                    },
                    /// Write sensor gain 1 (needs Bank1 sync)
                    /// The minimum total gain (gain1+gain2) setting is 2X.
                    register B_global {
                        type Access = WO;
                        const ADDRESS = 0x0B;
                        const SIZE_BITS = 5;
                        value: uint = 0..5,
                    },
                    #[doc = "Write sensor gain 2 (needs Bank1 sync)"]
                    register B_ggh {
                        type Access = WO;
                        const ADDRESS = 0x0C;
                        const SIZE_BITS = 2;
                        value: uint = 0..2,
                    },
                    #[doc = "Write sensor exposure time (needs Bank1 sync)"]
                    register B_expo {
                        type Access = WO;
                        const ADDRESS = 0x0F;
                        const SIZE_BITS = 16;
                        value: uint = 0..16,
                    },
                    #[doc = "DSP settings; area min threshold"]
                    register Cmd_oalb {
                        const ADDRESS = 0x46;
                        const SIZE_BITS = 8;
                        value: uint = 0..8,
                    },
                    #[doc = "DSP settings; brightness threshold"]
                    register Cmd_thd {
                        const ADDRESS = 0x47;
                        const SIZE_BITS = 8;
                        value: uint = 0..8,
                    },
                    #[doc = "DSP settings; x-axis Interpolated Resolution"]
                    register Cmd_scale_resolution_x {
                        const ADDRESS = 0x60;
                        const SIZE_BITS = 12;
                        value: uint = 0..12,
                    },
                    #[doc = "DSP settings; y-axis Interpolated Resolution"]
                    register Cmd_scale_resolution_y {
                        const ADDRESS = 0x62;
                        const SIZE_BITS = 12;
                        value: uint = 0..12,
                    }
                },
            },
            block Output {
                register Bank5 {
                    const ADDRESS = 0x0500;
                    const SIZE_BITS = 2048;
                    value: uint = 0..2048,
                }
            },
        }
    );
}

pub fn parse_bank5(bytes: [u8; 2048 / 8]) -> [ObjectFormat1; 16] {
    let mut out = [ObjectFormat1::DEFAULT; 16];
    for (i, c) in bytes.chunks_exact(16).enumerate() {
        let mut tmp = [0u8; 16];
        tmp.copy_from_slice(c);
        out[i] = bytemuck::pod_read_unaligned(&tmp); // `tmp` is only byte-aligned
    }
    out
}
