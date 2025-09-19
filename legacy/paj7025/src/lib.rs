#![no_std]

pub mod types;
mod u2048;

use device_driver::AsyncRegisterInterface;
use embedded_hal_async::spi::{Operation};

use crate::types::ObjectFormat1;

const REG_BANK_SELECT: u8 = 0xEF;     // Non-bank register for bank switching
const CMD_MULTI: u8 = 0x01;           // CMD[6:0] = 0x00 single, 0x01 multi
const CMD_READ:  u8 = 0x80;           // CMD bit7 = 1 => Read
const CMD_WRITE: u8 = 0x00;           // CMD bit7 = 0 => Write

#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Paj7025Error<SpiError> {
    Spi(SpiError),
}

pub struct DeviceInterface<SpiDevice> {
    spi_device: SpiDevice,
}

impl<I> DeviceInterface<I> {
    pub fn new(spi_device: I) -> Self {
        Self { spi_device }
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
        let reg:  u8 = (address & 0xFF) as u8;

        let cmd_bank = [CMD_WRITE | 0x00, REG_BANK_SELECT];
        let bank_val = [bank];
        let mut ops1 = [
            Operation::Write(&cmd_bank),
            Operation::Write(&bank_val),
        ];
        self.spi_device.transaction(&mut ops1).await.map_err(Paj7025Error::Spi)?;

        let cmd = CMD_READ | if data.len() > 1 { CMD_MULTI } else { 0 };
        let cmd_addr = [cmd, reg];
        let mut ops2 = [
            Operation::Write(&cmd_addr),
            Operation::TransferInPlace(data),
        ];
        self.spi_device.transaction(&mut ops2).await.map_err(Paj7025Error::Spi)?;
        Ok(())
    }

    async fn write_register(
        &mut self,
        address: u16,
        _size_bits: u32,
        data: &[u8],
    ) -> Result<(), Self::Error> {
        let bank: u8 = (address >> 8) as u8;
        let reg:  u8 = (address & 0xFF) as u8;

        let cmd_bank = [CMD_WRITE | 0x00, REG_BANK_SELECT];
        let bank_val = [bank];
        let mut ops1 = [
            Operation::Write(&cmd_bank),
            Operation::Write(&bank_val),
        ];
        self.spi_device.transaction(&mut ops1).await.map_err(Paj7025Error::Spi)?;

        let cmd = CMD_WRITE | if data.len() > 1 { CMD_MULTI } else { 0 };
        let cmd_addr = [cmd, reg];
        let mut ops2 = [
            Operation::Write(&cmd_addr),
            Operation::Write(data),
        ];
        self.spi_device.transaction(&mut ops2).await.map_err(Paj7025Error::Spi)?;
        Ok(())
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
    SpiDevice: embedded_hal_async::spi::SpiDevice<Error = E> + 'static,
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
                    /// unit = 200ns
                    /// Note: The minimum setting is 100.
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
                    /// Note: The minimum total gain (gain1+gain2) setting is 2X.
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

pub fn parse_bank5(bytes: [u8; 2048/8]) -> [ObjectFormat1; 16] {
    let mut arr = [ObjectFormat1::default(); 16];
    for (i, chunk) in bytes.chunks_exact(16).enumerate() {
        arr[i] = *bytemuck::from_bytes::<ObjectFormat1>(chunk);
    }
    arr
}
