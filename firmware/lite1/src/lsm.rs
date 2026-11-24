//! LSM6DSV320X low-g accelerometer and gyroscope adapter.

use defmt::Format;
use embassy_time::{Delay, Timer};
use embedded_hal::spi::{Operation, SpiDevice};
use embedded_hal_bus::spi::ExclusiveDevice;

use crate::object_mode::{ImuData, ImuSensor};

/// SPI device used by lite1 SERIAL1.
pub type LsmSpiDevice = ExclusiveDevice<
    embassy_nrf::spim::Spim<'static, embassy_nrf::mode::Async>,
    embassy_nrf::gpio::Output<'static>,
    Delay,
>;

const WHO_AM_I: u8 = 0x0f;
const EXPECTED_ID: u8 = 0x73;
const STATUS_REG: u8 = 0x1e;
const CTRL1_XL: u8 = 0x10;
const CTRL2_G: u8 = 0x11;
const CTRL3_C: u8 = 0x12;
const IF_CFG: u8 = 0x03;
const CTRL4: u8 = 0x13;
const CTRL6: u8 = 0x15;
const CTRL8: u8 = 0x17;
const INT1_CTRL: u8 = 0x0d;
const CTRL1_XL_HG: u8 = 0x4e;
const INTERNAL_FREQ_FINE: u8 = 0x4f;
const FUNCTIONS_ENABLE: u8 = 0x50;
const OUTX_L_G: u8 = 0x22;
const TIMESTAMP0: u8 = 0x40;
// DS14623 Rev 2 Table 3 and Tables 84-85, 152-153: sensitivities, status flags, timestamp rate.
const TIMESTAMP_RATE_HZ: u64 = 46_080;
const TIMESTAMP_FINE_SCALE: i64 = 13;
const ACCEL_MG_PER_LSB: f32 = 0.488;
const GYRO_DPS_PER_LSB: f32 = 0.07;
const G_MS2: f32 = 9.806_65;
const STATUS_XLDA_GDA: u8 = 0x03;
const STATUS_POLLS: u8 = 20;
const TIMESTAMP_SCALE_Q16: u64 = 10_000_000_000 * 65_536;

#[derive(Debug, Format)]
pub enum LsmError {
    Spi,
    WrongWhoAmI,
    ResetTimeout,
    NoFreshPair,
}

pub struct Lsm6dsv320xSensor {
    spi: LsmSpiDevice,
    timestamp_tick_us_q16: u32,
    last_timestamp_raw: u32,
    timestamp_fraction_q16: u16,
    timestamp_micros: u32,
}

impl Lsm6dsv320xSensor {
    pub fn standby(spi: &mut LsmSpiDevice) -> Result<(), LsmError> {
        check_id(spi)?;
        // Reset disables all ODRs, FIFO and interrupt routes after an MCU-only reset.
        reset(spi)
    }

    pub fn initialize(mut spi: LsmSpiDevice) -> Result<Self, LsmError> {
        check_id(&mut spi)?;
        reset(&mut spi)?;

        // DS14623 Rev 2 §§9.17, 9.28: latched, filter-settled DRDY for both sensors.
        for (register, clear, set) in [
            (CTRL3_C, 0, 0x44),
            (CTRL6, 0x07, 0x04),
            (CTRL8, 0x03, 0x03),
            (IF_CFG, 0, 0x10),
            (INT1_CTRL, 0xff, 0x03),
            (CTRL4, 0x02, 0x08),
            (FUNCTIONS_ENABLE, 0, 0x40),
            (CTRL1_XL, 0xff, 0x06),
            (CTRL2_G, 0xff, 0x06),
        ] {
            let mut value = [0];
            if clear != 0xff {
                read_regs(&mut spi, register, &mut value)?;
            }
            write_reg(&mut spi, register, (value[0] & !clear) | set)?;
        }

        let mut freq_fine = [0];
        read_regs(&mut spi, INTERNAL_FREQ_FINE, &mut freq_fine)?;
        let timestamp_tick_us_q16 = timestamp_tick_us_q16(freq_fine[0] as i8);
        let mut ts = [0; 4];
        read_regs(&mut spi, TIMESTAMP0, &mut ts)?;

        Ok(Self {
            spi,
            timestamp_tick_us_q16,
            last_timestamp_raw: u32::from_le_bytes(ts),
            timestamp_fraction_q16: 0,
            timestamp_micros: 0,
        })
    }
}

impl ImuSensor for Lsm6dsv320xSensor {
    type Error = LsmError;

    async fn read_data(&mut self) -> Result<ImuData, Self::Error> {
        let mut ready = false;
        for _ in 0..STATUS_POLLS {
            let mut status = [0];
            read_regs(&mut self.spi, STATUS_REG, &mut status)?;
            if pair_ready(status[0]) {
                ready = true;
                break;
            }
            Timer::after_micros(100).await;
        }
        if !ready {
            return Err(LsmError::NoFreshPair);
        }

        let mut ts = [0; 4];
        // Hardware counter snapshot after both fresh flags, immediately before output burst.
        read_regs(&mut self.spi, TIMESTAMP0, &mut ts)?;
        let raw_timestamp = u32::from_le_bytes(ts);
        let ticks = raw_timestamp.wrapping_sub(self.last_timestamp_raw);
        let (delta_us, fraction_q16) = elapsed_us(
            ticks,
            self.timestamp_tick_us_q16,
            self.timestamp_fraction_q16,
        );
        self.timestamp_micros = self.timestamp_micros.wrapping_add(delta_us);
        self.timestamp_fraction_q16 = fraction_q16;
        self.last_timestamp_raw = raw_timestamp;

        let mut bytes = [0; 12];
        read_regs(&mut self.spi, OUTX_L_G, &mut bytes)?;
        let gyro_raw = [
            read_i16(&bytes[0..2]),
            read_i16(&bytes[2..4]),
            read_i16(&bytes[4..6]),
        ];
        let accel_raw = [
            read_i16(&bytes[6..8]),
            read_i16(&bytes[8..10]),
            read_i16(&bytes[10..12]),
        ];
        let accel = accel_raw.map(acceleration_ms2);
        let gyro = gyro_raw.map(angular_rate_rads);
        Ok(ImuData {
            accel: board_frame(accel),
            gyro: board_frame(gyro),
            timestamp_micros: self.timestamp_micros,
        })
    }
}

// Blocking register bursts (at most 13 bytes) avoid oversized async state machines.
fn read_regs(spi: &mut LsmSpiDevice, register: u8, bytes: &mut [u8]) -> Result<(), LsmError> {
    let address = [register | 0x80];
    spi.transaction(&mut [Operation::Write(&address), Operation::Read(bytes)])
        .map_err(|_| LsmError::Spi)
}

fn write_reg(spi: &mut LsmSpiDevice, register: u8, value: u8) -> Result<(), LsmError> {
    let bytes = [register, value];
    spi.transaction(&mut [Operation::Write(&bytes)])
        .map_err(|_| LsmError::Spi)
}

fn check_id(spi: &mut LsmSpiDevice) -> Result<(), LsmError> {
    let mut id = [0];
    read_regs(spi, WHO_AM_I, &mut id)?;
    if id[0] == EXPECTED_ID {
        Ok(())
    } else {
        Err(LsmError::WrongWhoAmI)
    }
}

fn reset(spi: &mut LsmSpiDevice) -> Result<(), LsmError> {
    for register in [CTRL1_XL, CTRL2_G, CTRL1_XL_HG] {
        write_reg(spi, register, 0)?;
    }
    write_reg(spi, CTRL3_C, 0x01)?;
    for attempt in 0..4 {
        let mut ctrl3 = [0];
        read_regs(spi, CTRL3_C, &mut ctrl3)?;
        if ctrl3[0] & 0x01 == 0 {
            return Ok(());
        }
        if attempt < 3 {
            embassy_time::block_for(embassy_time::Duration::from_micros(150));
        }
    }
    Err(LsmError::ResetTimeout)
}

fn timestamp_tick_us_q16(freq_fine: i8) -> u32 {
    let scale = 10_000 + TIMESTAMP_FINE_SCALE * i64::from(freq_fine);
    let denominator = TIMESTAMP_RATE_HZ * scale as u64;
    ((TIMESTAMP_SCALE_Q16 + denominator / 2) / denominator) as u32
}
fn pair_ready(status: u8) -> bool {
    status & STATUS_XLDA_GDA == STATUS_XLDA_GDA
}

fn elapsed_us(ticks: u32, tick_us_q16: u32, fraction_q16: u16) -> (u32, u16) {
    let elapsed_q16 = u64::from(ticks) * u64::from(tick_us_q16) + u64::from(fraction_q16);
    ((elapsed_q16 >> 16) as u32, elapsed_q16 as u16)
}

fn read_i16(bytes: &[u8]) -> i16 {
    i16::from_le_bytes([bytes[0], bytes[1]])
}

fn acceleration_ms2(raw: i16) -> f32 {
    raw as f32 * ACCEL_MG_PER_LSB * 0.001 * G_MS2
}

fn angular_rate_rads(raw: i16) -> f32 {
    (raw as f32 * GYRO_DPS_PER_LSB).to_radians()
}

// TODO: Assumes LSM6DSV320X axes map to board [y, x, -z]; verify PCB orientation or six-axis hardware measurement.
fn board_frame([x, y, z]: [f32; 3]) -> [f32; 3] {
    [y, x, -z]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_sensor_ranges_and_board_axes() {
        let accel_x = acceleration_ms2(i16::MAX);
        let accel_z = acceleration_ms2(2048);
        let gyro_x = angular_rate_rads(28_571);
        assert!((accel_x - 156.811_24).abs() < 0.000_1);
        assert!((gyro_x.to_degrees() - 2000.0).abs() < 0.1);
        assert_eq!(
            board_frame([accel_x, 0.0, accel_z]),
            [0.0, accel_x, -accel_z]
        );
        assert_eq!(board_frame([gyro_x, 0.0, 0.0]), [0.0, gyro_x, 0.0]);
    }

    #[test]
    fn decodes_little_endian_signed_sample() {
        assert_eq!(read_i16(&[0x00, 0x80]), i16::MIN);
        assert_eq!(read_i16(&[0xff, 0x7f]), i16::MAX);
    }

    #[test]
    fn requires_fresh_accel_and_gyro_flags() {
        assert!(!pair_ready(0));
        assert!(!pair_ready(0x01));
        assert!(!pair_ready(0x02));
        assert!(pair_ready(0x03));
    }

    #[test]
    fn timestamp_counter_wrap_and_fine_calibration() {
        let delta = 3u32.wrapping_sub(u32::MAX - 1);
        let tick_scale = timestamp_tick_us_q16(0);
        let (elapsed, _) = elapsed_us(delta, tick_scale, 0);
        assert_eq!(delta, 5);
        assert_eq!(elapsed, 108);
        assert!(timestamp_tick_us_q16(100) < tick_scale);
    }
}
