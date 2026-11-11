use crate::imu::{AtsliteImu, AtsliteImuInterrupt};
use crate::l2cap::AtsliteL2capChannels;

/// Since: Odyssey Desktop 0.7.0.
pub struct AtslitePlatform;

impl legacy_common::platform::Platform for AtslitePlatform {
    type Imu = AtsliteImu;
    type ImuInterrupt = AtsliteImuInterrupt;
    type L2capChannels = AtsliteL2capChannels;
    type Timer = embassy_nrf::peripherals::TIMER0;

    const NAME: &'static str = "ATSlite (nRF5340)";

    // Scale carried over from the earlier firmware. It does not match the
    // ICM42688's 16-bit table (±2 g is 16384 LSB/g, ±16 g is 2048), so check it
    // against the configured range and the 20-bit FIFO packets.
    const ACC_LSB_PER_G: f32 = 32768.0;

    // Carried over from the earlier firmware: 262.144 LSB/(deg/s) is the ±125 dps
    // scale, while ±250 dps would be 131.072.
    const GYRO_LSB_PER_DPS: f32 = 262.144;

    // ICM42688 mounting: keep X, flip Y and Z.
    fn transform_accel(raw: [i32; 3]) -> [i32; 3] {
        [raw[0], -raw[1], -raw[2]]
    }

    fn transform_gyro(raw: [i32; 3]) -> [i32; 3] {
        [raw[0], -raw[1], -raw[2]]
    }

    fn round_accel_odr(hz: u16) -> u16 {
        crate::imu::round_accel_odr_hz(hz)
    }

    fn device_id() -> [u8; 8] {
        crate::utils::device_id()
    }

    fn netcore_release() -> u16 {
        crate::ble::netcore::netcore_release()
    }
}
