// ATSlite Platform trait implementation

use crate::imu::{AtsliteImu, AtsliteImuInterrupt};
use crate::l2cap::AtsliteL2capChannels;

pub struct AtslitePlatform;

impl common::platform::Platform for AtslitePlatform {
    type Imu = AtsliteImu;
    type ImuInterrupt = AtsliteImuInterrupt;
    type L2capChannels = AtsliteL2capChannels;
    type Timer = embassy_nrf::peripherals::TIMER0;

    const NAME: &'static str = "ATSlite (nRF5340)";

    // ICM42688 with ±16g range (default): 2048 LSB/g (same as BMI, actually uses 32768 for ±2g but we use ±16g)
    // Actually checking the old code: it used 32768 LSB/g, which means ±2g range
    const ACC_LSB_PER_G: f32 = 32768.0;

    // ICM42688 with ±250 dps range (default): 131.072 LSB/(deg/s) for ±250dps
    // Old code used 262.144 which is for ±125dps range
    const GYRO_LSB_PER_DPS: f32 = 262.144;

    // ICM42688 axis transforms: keep X, flip Y and Z
    fn transform_accel(raw: [i32; 3]) -> [i32; 3] {
        [raw[0], -raw[1], -raw[2]]
    }

    fn transform_gyro(raw: [i32; 3]) -> [i32; 3] {
        [raw[0], -raw[1], -raw[2]]
    }

    fn device_id() -> [u8; 8] {
        crate::utils::device_id()
    }
}
